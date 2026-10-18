// SPDX-License-Identifier: GPL-3.0-or-later
//! `org.freedesktop.Notifications`, on zbus's thread.
//!
//! The shape is the tray's and `wlrix-idle`'s: zbus dispatches method calls on a thread of its
//! own, each call is turned into a [`BusEvent`] and sent down a `calloop::channel` to the main
//! loop, and the loop owns every piece of state. No async runtime touches the loop.
//!
//! # What has to be answered on this side
//!
//! `Notify` returns the notification's id, so the id is allocated here, before the loop has seen
//! anything -- a sender that calls `Notify` and then `CloseNotification` straight away must get an
//! id the loop will recognize, and handing that out cannot wait on the loop. Everything else the
//! loop decides.
//!
//! The converting happens here too: hints are read into [`Hints`], the body is parsed, and the
//! actions are paired, so a malformed notification costs this thread a few microseconds and the
//! loop nothing.
//!
//! # Signals go out from the loop
//!
//! `NotificationClosed` and `ActionInvoked` are emitted by the loop, through [`Bus`], when it
//! closes something or someone clicks. Emitting a signal does not wait for anyone, so doing it
//! from the loop cannot stall the Wayland connection the way a method call could.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

use zbus::object_server::SignalEmitter;
use zbus::zvariant::OwnedValue;

use crate::notification::{self, Hints, Notification, Reason};

/// The well-known name. Whoever owns it is the notification server.
pub const NAME: &str = "org.freedesktop.Notifications";
/// Where the interface is served. Fixed by the specification.
pub const PATH: &str = "/org/freedesktop/Notifications";
/// The interface, which has the same name as the bus name.
pub const INTERFACE: &str = "org.freedesktop.Notifications";

/// What this server can do, as `GetCapabilities` says it.
///
/// Only what is actually done. `body-hyperlinks` and `body-images` are absent because links are
/// not clickable and images are drawn as their alt text (see [`crate::markup`]); `persistence`
/// because there is no history yet; `sound` because nothing is played yet. A sender that sees a
/// capability missing falls back gracefully, which is the point of asking -- claiming one that is
/// not there gets a body full of markup nobody acts on.
///
/// `x-canonical-private-synchronous` is not a capability in the specification. It is what
/// notify-osd advertised for its stack tag, and some senders look for it before using the hint.
pub const CAPABILITIES: &[&str] = &[
    "actions",
    "body",
    "body-markup",
    "icon-static",
    "x-canonical-private-synchronous",
];

/// What the bus tells the main loop.
#[derive(Debug)]
pub enum BusEvent {
    Notify(Box<Notification>),
    Close(u32),
    /// Another daemon took the name with `--replace` or its own equivalent. Nothing will be sent
    /// here again, so the loop should go.
    Lost,
}

/// The interface object.
struct Server {
    sender: calloop::channel::Sender<BusEvent>,
    /// The last id handed out. Ids start at 1; 0 means "none" on the wire.
    last_id: AtomicU32,
}

impl Server {
    /// The next free id, skipping 0 when the counter wraps -- four billion notifications later,
    /// but a server that answered 0 would be telling its sender the notification was not shown.
    fn next_id(&self) -> u32 {
        loop {
            let id = self.last_id.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
            if id != 0 {
                return id;
            }
        }
    }
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    /// Show a notification, or replace one.
    ///
    /// With a non-zero `replaces_id` the answer is that id, whether or not the notification is
    /// still open: the specification says so, and a sender updating a progress notification that
    /// someone dismissed expects its next update to come back under the same id rather than start
    /// a new series.
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            self.next_id()
        };
        let notification = Notification {
            id,
            app_name,
            app_icon,
            summary,
            body: crate::markup::parse(&body),
            actions: notification::actions(&actions),
            hints: Hints::read(&hints),
            expire_timeout,
        };
        // A closed channel means the loop is on its way out; the sender still gets its id.
        let _ = self.sender.send(BusEvent::Notify(Box::new(notification)));
        id
    }

    /// Close a notification. One that is not open is not an error: it has usually just expired,
    /// and the specification's "empty error" for that case is something no client handles.
    fn close_notification(&self, id: u32) {
        let _ = self.sender.send(BusEvent::Close(id));
    }

    fn get_capabilities(&self) -> Vec<String> {
        CAPABILITIES.iter().map(|cap| cap.to_string()).collect()
    }

    /// Name, vendor, version, and the version of the specification followed.
    fn get_server_information(&self) -> (String, String, String, String) {
        (
            "wlrix-osd".to_owned(),
            "wlRIX".to_owned(),
            env!("CARGO_PKG_VERSION").to_owned(),
            "1.2".to_owned(),
        )
    }

    // Declared for introspection, so `busctl introspect` and `gdbus` show the whole interface.
    // They are emitted by hand from the loop, through `Bus`, which owns no `SignalEmitter`.
    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;
}

/// The bus, once it is up: what the main loop keeps hold of to emit signals.
pub struct Bus {
    connection: zbus::blocking::Connection,
}

impl Bus {
    /// Tell everyone a notification has gone, and why.
    pub fn closed(&self, id: u32, reason: Reason) {
        self.emit("NotificationClosed", &(id, reason as u32));
    }

    /// Tell everyone an action was chosen.
    pub fn action_invoked(&self, id: u32, key: &str) {
        self.emit("ActionInvoked", &(id, key));
    }

    /// Broadcast rather than sent to the notification's sender: the specification says only that
    /// the signal is emitted, and libnotify listens for it with a match rule rather than as a
    /// unicast. A failure is logged and dropped -- the bus going away is the session ending.
    fn emit<B: serde::Serialize + zbus::zvariant::DynamicType>(&self, signal: &str, body: &B) {
        if let Err(err) = self.connection.emit_signal(
            None::<zbus::names::BusName<'_>>,
            PATH,
            INTERFACE,
            signal,
            body,
        ) {
            eprintln!("wlrix-osd: could not emit {signal}: {err}");
        }
    }
}

/// Serve the interface and take the name.
///
/// Fails rather than carrying on if the name cannot be had. A notification daemon that is
/// running but is not the one notifications go to is worse than none: everything would still
/// appear, from mako, and the user would be looking at the wrong program's settings for why.
pub fn spawn(replace: bool) -> Result<(Bus, calloop::channel::Channel<BusEvent>), String> {
    let (sender, channel) = calloop::channel::channel();
    let lost = sender.clone();

    let connection = zbus::blocking::connection::Builder::session()
        .map_err(|err| format!("no session bus: {err}"))?
        // Served before the name is requested, so a sender that calls the instant the name
        // appears never finds an empty connection behind it.
        .serve_at(
            PATH,
            Server {
                sender,
                last_id: AtomicU32::new(0),
            },
        )
        .map_err(|err| format!("could not serve {INTERFACE}: {err}"))?
        .build()
        .map_err(|err| format!("could not connect to the session bus: {err}"))?;

    // `DoNotQueue` and `AllowReplacement`, always, for the tray's reasons: queueing would leave
    // this running and unreachable, and replacement is granted by the *incumbent* -- a name taken
    // without the flag can never be taken over, so a later `--replace` could not work.
    let mut flags =
        zbus::fdo::RequestNameFlags::DoNotQueue | zbus::fdo::RequestNameFlags::AllowReplacement;
    if replace {
        flags |= zbus::fdo::RequestNameFlags::ReplaceExisting;
    }
    let taken = || {
        format!(
            "{NAME} is already owned by another notification daemon -- mako or dunst, most \
             likely, started by D-Bus activation. Stop it and run this again, or pass --replace. \
             Uninstalling it, or removing its D-Bus service file, stops it coming back."
        )
    };
    // zbus answers a refusal with `NameTaken` rather than handing back the reply, so both paths
    // lead to the same explanation.
    let reply = match connection.request_name_with_flags(NAME, flags) {
        Ok(reply) => reply,
        Err(zbus::Error::NameTaken) => return Err(taken()),
        Err(err) => return Err(format!("could not request {NAME}: {err}")),
    };
    if !matches!(
        reply,
        zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner
    ) {
        return Err(taken());
    }

    watch_for_replacement(&connection, lost)?;
    Ok((Bus { connection }, channel))
}

/// Tell the loop when the name is taken away.
///
/// `AllowReplacement` means a later daemon started with `--replace` gets the name without asking
/// this one. Carrying on after that would leave a process drawing notifications nobody can send it
/// any more, and holding the last few on screen forever. `NameLost` is sent to the loser alone, so
/// one blocking iterator on its own thread is all it takes.
fn watch_for_replacement(
    connection: &zbus::blocking::Connection,
    sender: calloop::channel::Sender<BusEvent>,
) -> Result<(), String> {
    let proxy = zbus::blocking::fdo::DBusProxy::new(connection)
        .map_err(|err| format!("could not reach the bus daemon: {err}"))?;
    let lost = proxy
        .receive_name_lost()
        .map_err(|err| format!("could not watch for NameLost: {err}"))?;
    std::thread::Builder::new()
        .name("name-lost".into())
        .spawn(move || {
            for signal in lost {
                if signal.args().is_ok_and(|args| args.name() == NAME) {
                    let _ = sender.send(BusEvent::Lost);
                    return;
                }
            }
        })
        .map_err(|err| format!("could not start the NameLost thread: {err}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        let (sender, _channel) = calloop::channel::channel();
        Server {
            sender,
            last_id: AtomicU32::new(0),
        }
    }

    #[test]
    fn ids_start_at_one_and_never_come_back_as_zero() {
        let server = server();
        assert_eq!(server.next_id(), 1);
        assert_eq!(server.next_id(), 2);
        server.last_id.store(u32::MAX - 1, Ordering::Relaxed);
        assert_eq!(server.next_id(), u32::MAX);
        assert_eq!(server.next_id(), 1, "0 is skipped on the wrap");
    }

    #[test]
    fn a_replacement_keeps_the_id_it_asked_for() {
        let server = server();
        let id = server.notify(
            "app".into(),
            41,
            String::new(),
            "s".into(),
            String::new(),
            Vec::new(),
            HashMap::new(),
            -1,
        );
        assert_eq!(id, 41);
        // ...and did not use one up.
        assert_eq!(server.next_id(), 1);
    }

    #[test]
    fn only_honest_capabilities_are_claimed() {
        for absent in ["body-hyperlinks", "body-images", "persistence", "sound"] {
            assert!(!CAPABILITIES.contains(&absent), "{absent}");
        }
    }
}
