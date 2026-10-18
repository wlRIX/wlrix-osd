// SPDX-License-Identifier: GPL-3.0-or-later
//! Notifications, as a Wayland client.
//!
//! One **wlr-layer-shell overlay surface**, anchored to a corner, exactly as big as the stack it
//! draws. `Overlay` and not `Top`: a notification has to be seen over whatever is on screen,
//! including the toolchest and anything else on the top layer. It takes **no exclusive zone** and
//! **no keyboard** -- it must never take focus from what someone is typing into.
//!
//! `calloop` drives everything: the Wayland connection, the channel the bus reports on, and the
//! `SIGHUP` reload ping are sources on one loop, and the loop's timeout is the next expiry, so
//! nothing polls.
//!
//! # One surface, with the gaps cut out of its input region
//!
//! The stack is drawn into a single surface rather than one per notification. The gaps between
//! notifications are transparent, and they are also left out of the surface's **input region**,
//! so a click in a gap reaches the window underneath. That works because of how
//! `wlrix-compositor` hit-tests the overlay layer: it picks the topmost overlay surface by
//! bounding box, and when that surface's input region rejects the point it moves on to the top
//! layer and then to the windows. (The one thing that does not happen is falling through to a
//! *second* overlay surface under the first; nothing else of wlRIX's is on that layer.)
//!
//! The tray's two rules hold here for the same reasons: nothing is attached before the surface
//! is configured, and the surface is resized to exactly what is drawn.
//!
//! # Coming back from a blank
//!
//! Every layer surface is closed when the monitors blank, which is an output being removed. The
//! notifications are not: they stay in the [`Stack`], those not yet shown keep their clocks
//! stopped (see [`crate::stack`]), and the surface is rebuilt when an output returns.

pub mod paint;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    reexports::calloop::EventLoop,
    reexports::calloop_wayland_source::WaylandSource,
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor as ShellAnchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler,
            LayerSurface, LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, QueueHandle,
    backend::WaylandError,
    globals::registry_queue_init,
    protocol::{
        wl_output::WlOutput, wl_pointer::WlPointer, wl_seat::WlSeat, wl_shm, wl_surface::WlSurface,
    },
};
use wlrix_ui::canvas::Canvas;
use wlrix_ui::palette::Palette;
use wlrix_ui::text::{Face, Fonts};

use crate::config::{Config, Metrics};
use crate::dbus::{Bus, BusEvent};
use crate::icons::{DesktopEntries, Icons};
use crate::layout::{Anchor, Bubble, Content, Frame, Text};
use crate::notification::{Notification, Reason, Urgency};
use crate::pixmap::Pixmap;
use crate::stack::{Added, Stack};

/// `wl_pointer`'s buttons, from `linux/input-event-codes.h`.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// The title when neither the sender nor its `.desktop` file gave a name.
const UNNAMED: &str = "Notification";

/// What a notification looks like, worked out once when it arrives rather than every frame.
struct Look {
    title: String,
    picture: Option<Pixmap>,
}

/// Which part of a notification a press landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    /// The window-menu button, which closes it.
    Button,
    /// Anywhere else on it.
    Body,
}

/// The whole daemon's state.
pub struct Osd {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    shm: Shm,
    compositor: CompositorState,
    layer_shell: LayerShell,
    pool: SlotPool,

    /// The stack's surface, while there is anything in it.
    layer: Option<LayerSurface>,
    output: Option<WlOutput>,
    /// That output's logical size, which caps how tall the stack may grow.
    output_size: Option<(i32, i32)>,
    configured: bool,
    width: u32,
    height: u32,
    requested: (u32, u32),

    pointer: Option<WlPointer>,

    fonts: Fonts,
    icons: Icons,
    desktop_entries: DesktopEntries,
    config: Config,
    palette: &'static Palette,
    metrics: Metrics,
    anchor: Anchor,

    bus: Bus,
    stack: Stack,
    looks: HashMap<u32, Look>,

    /// The notifications drawn this frame, in frame order, with their layouts.
    shown: Vec<(u32, Bubble)>,
    frame: Frame,

    /// Where the pointer is on the surface, and over which notification.
    pointer_at: Option<(i32, i32)>,
    hovered: Option<u32>,
    pressed: Option<(u32, Part, u32)>,

    dirty: bool,
    exit: bool,
}

/// Run until the compositor goes away.
pub fn run(replace: bool) -> Result<(), String> {
    let config = Config::load();
    let palette = resolve_palette(&config);
    let fonts = Fonts::load()?;
    let mut icons = Icons::default();
    icons.set_theme(config.appearance.icon_theme());

    // The bus first: without the name, nothing is ever sent here, and a daemon drawing nothing
    // while mako draws everything is the confusing failure. This is the one worth refusing to
    // start over.
    let (bus, events) = crate::dbus::spawn(replace)?;
    eprintln!(
        "wlrix-osd: serving {} on the session bus, text in {} ({} faces), scheme {}",
        crate::dbus::NAME,
        fonts.family(),
        fonts.face_count(),
        palette.id,
    );

    let conn = Connection::connect_to_env()
        .map_err(|err| format!("no Wayland compositor to connect to: {err}"))?;
    let (globals, event_queue) =
        registry_queue_init(&conn).map_err(|err| format!("could not read the registry: {err}"))?;
    let qh = event_queue.handle();

    let mut event_loop: EventLoop<Osd> =
        EventLoop::try_new().map_err(|err| format!("could not create the event loop: {err}"))?;
    let loop_handle = event_loop.handle();

    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|err| format!("wl_compositor unavailable: {err}"))?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .map_err(|err| format!("wlr-layer-shell unavailable: {err}"))?;
    let shm = Shm::bind(&globals, &qh).map_err(|err| format!("wl_shm unavailable: {err}"))?;
    // Five notifications of a few hundred pixels square each. The pool grows if a stack wants
    // more.
    let pool = SlotPool::new(400 * 800 * 4, &shm)
        .map_err(|err| format!("could not create a buffer pool: {err}"))?;

    let metrics = config.metrics();
    let anchor = config.anchor;
    let mut osd = Osd {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        seat_state: SeatState::new(&globals, &qh),
        shm,
        compositor,
        layer_shell,
        pool,
        layer: None,
        output: None,
        output_size: None,
        configured: false,
        width: 0,
        height: 0,
        requested: (0, 0),
        pointer: None,
        fonts,
        icons,
        desktop_entries: DesktopEntries::default(),
        config,
        palette,
        metrics,
        anchor,
        bus,
        stack: Stack::default(),
        looks: HashMap::new(),
        shown: Vec::new(),
        frame: Frame::default(),
        pointer_at: None,
        hovered: None,
        pressed: None,
        dirty: false,
        exit: false,
    };

    let health = conn.clone();
    WaylandSource::new(conn, event_queue)
        .insert(loop_handle.clone())
        .map_err(|err| format!("could not drive Wayland from the loop: {err}"))?;

    let qh_for_events = qh.clone();
    loop_handle
        .insert_source(events, move |event, _, osd: &mut Osd| {
            if let calloop::channel::Event::Msg(event) = event {
                osd.on_bus(&qh_for_events, event);
            }
        })
        .map_err(|err| format!("could not listen to the bus: {err}"))?;

    let (reload_ping, reload_source) = calloop::ping::make_ping()
        .map_err(|err| format!("could not create the reload ping: {err}"))?;
    let qh_for_reload = qh.clone();
    loop_handle
        .insert_source(reload_source, move |_, _, osd: &mut Osd| {
            osd.reload_config(&qh_for_reload);
        })
        .map_err(|err| format!("could not watch for reloads: {err}"))?;
    crate::signals::forward_reload_to_loop(reload_ping);

    let _pidfile = crate::pidfile::write();

    event_loop
        .dispatch(Duration::from_millis(200), &mut osd)
        .map_err(|err| format!("initial dispatch failed: {err}"))?;

    loop {
        // Asleep until something happens or the next notification is due to go. Nothing else
        // needs waking up for.
        let timeout = osd
            .stack
            .next_deadline()
            .map(|deadline| deadline.saturating_duration_since(Instant::now()));
        if let Err(err) = event_loop.dispatch(timeout, &mut osd) {
            return match health.flush() {
                // The compositor going away is the ordinary end of a Wayland client's life.
                Err(WaylandError::Io(_)) => Ok(()),
                _ => Err(format!("event loop failed: {err}")),
            };
        }
        // `calloop-wayland-source` drops protocol errors on the floor and leaves the socket
        // readable forever; caught here, as in the tray, or the loop spins silently.
        if let Some(err) = health.protocol_error() {
            return Err(format!("protocol error: {err}"));
        }
        osd.expire(&qh);
        osd.draw_if_dirty();
        if osd.exit {
            return Ok(());
        }
    }
}

/// The scheme the config names, said out loud when it names nothing real.
fn resolve_palette(config: &Config) -> &'static Palette {
    let (palette, unknown) = wlrix_ui::palette::resolve(config.appearance.palette.as_deref());
    if let Some(wlrix_ui::palette::UnknownPalette(name)) = unknown {
        eprintln!(
            "wlrix-osd: no color scheme called {name:?}; using {}",
            palette.id
        );
    }
    palette
}

impl Osd {
    /// Act on one thing the bus said.
    fn on_bus(&mut self, qh: &QueueHandle<Self>, event: BusEvent) {
        match event {
            BusEvent::Notify(notification) => self.open(*notification),
            BusEvent::Close(id) => {
                if self.stack.remove(id).is_some() {
                    self.looks.remove(&id);
                    self.bus.closed(id, Reason::Closed);
                }
            }
            BusEvent::Lost => {
                eprintln!(
                    "wlrix-osd: another notification daemon took {}; exiting",
                    crate::dbus::NAME
                );
                self.exit = true;
                return;
            }
        }
        self.refresh(qh);
    }

    /// Put a notification in the stack, or update the one it replaces.
    fn open(&mut self, notification: Notification) {
        let id = notification.id;
        eprintln!(
            "wlrix-osd: #{id} from {:?}: {:?}",
            notification.app_name, notification.summary
        );
        let lifetime = self
            .config
            .expiry(notification.hints.urgency, notification.expire_timeout);
        let look = self.look(&notification);
        self.looks.insert(id, look);
        if let Added::Displaced(old) = self.stack.add(notification, lifetime) {
            self.looks.remove(&old);
            // Not "dismissed" or "expired": nobody did anything to it. The specification's
            // undefined reason is the honest one for a notification superseded by its own sender.
            self.bus.closed(old, Reason::Undefined);
        }
    }

    /// The title and the picture.
    ///
    /// The picture is the first of these that resolves, in the order the specification gives:
    /// `image-data`, `image-path`, `app_icon`, the deprecated `icon_data` -- and then, past the
    /// specification, the icon in the sender's `.desktop` file, which is what an application that
    /// named a `desktop-entry` and nothing else is plainly asking for.
    fn look(&mut self, notification: &Notification) -> Look {
        let size = self.metrics.icon;
        let hints = &notification.hints;
        let entry = hints
            .desktop_entry
            .as_deref()
            .and_then(|id| self.desktop_entries.get(id))
            .cloned();

        let picture = hints
            .image
            .as_ref()
            .map(|image| image.fit(size))
            .or_else(|| {
                hints
                    .image_path
                    .as_deref()
                    .and_then(|path| self.icons.picture(path, size))
            })
            .or_else(|| self.icons.picture(&notification.app_icon, size))
            .or_else(|| hints.icon_data.as_ref().map(|image| image.fit(size)))
            .or_else(|| {
                entry
                    .as_ref()
                    .and_then(|entry| entry.icon.as_deref())
                    .and_then(|icon| self.icons.picture(icon, size))
            });

        let title = [
            Some(notification.app_name.as_str()),
            entry.as_ref().and_then(|entry| entry.name.as_deref()),
        ]
        .into_iter()
        .flatten()
        .find(|name| !name.trim().is_empty())
        .unwrap_or(UNNAMED)
        .to_owned();

        Look { title, picture }
    }

    /// Close everything whose time is up.
    fn expire(&mut self, qh: &QueueHandle<Self>) {
        let expired = self.stack.expire(Instant::now());
        if expired.is_empty() {
            return;
        }
        for id in expired {
            self.looks.remove(&id);
            self.bus.closed(id, Reason::Expired);
        }
        self.refresh(qh);
    }

    /// Close one because someone dismissed it.
    fn dismiss(&mut self, id: u32) {
        if self.stack.remove(id).is_some() {
            self.looks.remove(&id);
            self.bus.closed(id, Reason::Dismissed);
        }
    }

    /// Lay the stack out, resize the surface if it changed, and ask for a paint.
    ///
    /// The one place the stack's size is decided, so "one arrived", "one went" and "the config
    /// changed" all take the same path to the screen.
    fn refresh(&mut self, qh: &QueueHandle<Self>) {
        self.refresh_avoiding(qh, None);
    }

    /// [`Osd::refresh`], never building the surface on `going` -- for `output_destroyed`, which
    /// sctk calls while the output is still in its list.
    fn refresh_avoiding(&mut self, qh: &QueueHandle<Self>, going: Option<&WlOutput>) {
        if self.stack.is_empty() {
            // Nothing to show. A surface cannot be zero-sized, and an empty one would still be
            // in the way, so it goes.
            self.shown.clear();
            self.frame = Frame::default();
            if self.layer.take().is_some() {
                self.forget_surface();
            }
            return;
        }

        // The surface has to exist to know which output it is on, and so how tall it may be.
        self.ensure_surface(qh, going);

        let max = self.config.max_visible();
        let metrics = self.metrics;
        let Osd {
            stack,
            looks,
            fonts,
            ..
        } = self;
        let mut text = Measured { fonts };
        let mut laid_out: Vec<(u32, Bubble)> = stack
            .visible(max)
            .map(|entry| {
                let notification = &entry.notification;
                let look = looks.get(&entry.id());
                let content = Content {
                    title: look.map_or(UNNAMED, |look| look.title.as_str()),
                    summary: &notification.summary,
                    body: &notification.body,
                    picture: look
                        .and_then(|look| look.picture.as_ref())
                        .map(|picture| (picture.width, picture.height)),
                    value: notification.hints.value,
                };
                let bubble = Bubble::new(
                    &content,
                    metrics.width,
                    metrics.icon,
                    metrics.body_lines,
                    &mut text,
                );
                (entry.id(), bubble)
            })
            .collect();

        let heights: Vec<i32> = laid_out.iter().map(|(_, bubble)| bubble.height()).collect();
        let max_height = self
            .output_size
            .map_or(i32::MAX, |(_, height)| height - 2 * metrics.margin);
        let (frame, fitted) = Frame::new(
            &heights,
            metrics.width,
            metrics.gap,
            self.stack.waiting(max),
            self.anchor,
            max_height,
        );
        laid_out.truncate(fitted);
        self.shown = laid_out;
        self.frame = frame;

        // The pointer may now be over something else, or over nothing, without having moved.
        self.update_hover();

        let wanted = (self.frame.width as u32, self.frame.height as u32);
        if wanted != self.requested
            && let Some(layer) = self.layer.as_ref()
        {
            layer.set_size(wanted.0, wanted.1);
            layer.commit();
            self.requested = wanted;
            // No paint until the configure for the new size: a buffer of the old size on a
            // surface being resized is how a stack ends up clipped.
            return;
        }
        self.dirty = true;
    }

    /// The output the stack goes on: the configured one, else the one at the origin, else any.
    fn pick_output(&self, going: Option<&WlOutput>) -> Option<WlOutput> {
        let outputs: Vec<WlOutput> = self
            .output_state
            .outputs()
            .filter(|output| Some(output) != going)
            .collect();
        if let Some(wanted) = self.config.output.as_deref() {
            let named = outputs.iter().find(|output| {
                self.output_state
                    .info(output)
                    .and_then(|info| info.name)
                    .is_some_and(|name| name == wanted)
            });
            if let Some(output) = named {
                return Some(output.clone());
            }
            eprintln!("wlrix-osd: no output named {wanted:?}; using the default");
        }
        outputs
            .iter()
            .find(|output| {
                self.output_state
                    .info(output)
                    .and_then(|info| info.logical_position)
                    .is_some_and(|(x, y)| x == 0 && y == 0)
            })
            .or_else(|| outputs.first())
            .cloned()
    }

    /// Create the stack's surface, once there is an output other than `going` to put it on.
    fn ensure_surface(&mut self, qh: &QueueHandle<Self>, going: Option<&WlOutput>) {
        if self.layer.is_some() {
            return;
        }
        let Some(output) = self.pick_output(going) else {
            return;
        };
        self.output_size = self
            .output_state
            .info(&output)
            .and_then(|info| info.logical_size);

        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Overlay,
            Some("wlrix-osd"),
            Some(&output),
        );
        layer.set_anchor(shell_anchor(self.anchor));
        layer.set_exclusive_zone(0);
        // Never: a notification that took the keyboard would take it from whatever someone was
        // typing into, at a moment they did not choose.
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        set_margin(&layer, self.anchor, self.metrics.margin);
        // A size has to be asked for before the first commit; the real one follows from
        // `refresh` as soon as the layout is known.
        let first = (self.metrics.width as u32, 1);
        layer.set_size(first.0, first.1);
        layer.commit();

        self.layer = Some(layer);
        self.output = Some(output);
        self.requested = first;
        self.configured = false;
    }

    /// Throw away the surface, so the next [`Osd::refresh`] builds a fresh one.
    fn forget_surface(&mut self) {
        self.layer = None;
        self.output = None;
        self.output_size = None;
        self.configured = false;
        self.width = 0;
        self.height = 0;
        self.requested = (0, 0);
        self.pointer_at = None;
        self.hovered = None;
        self.pressed = None;
        self.stack.hover(None, Instant::now());
    }

    /// Re-read `osd.toml` and apply it.
    fn reload_config(&mut self, qh: &QueueHandle<Self>) {
        self.config = Config::load();
        self.metrics = self.config.metrics();
        self.palette = resolve_palette(&self.config);
        self.icons.set_theme(self.config.appearance.icon_theme());
        self.icons.clear();
        self.desktop_entries.clear();

        // The corner is set on the surface, so moving it needs a new one.
        if self.anchor != self.config.anchor {
            self.anchor = self.config.anchor;
            if self.layer.take().is_some() {
                self.forget_surface();
            }
        } else if let Some(layer) = self.layer.as_ref() {
            set_margin(layer, self.anchor, self.metrics.margin);
        }

        // The picture size may have changed, and the theme may find different pictures.
        let open: Vec<Notification> = self
            .stack
            .visible(usize::MAX)
            .map(|entry| entry.notification.clone())
            .collect();
        for notification in open {
            let look = self.look(&notification);
            self.looks.insert(notification.id, look);
        }
        self.refresh(qh);
    }

    /// Work out which notification the pointer is over, and hold that one's clock.
    fn update_hover(&mut self) {
        let hovered = self.pointer_at.and_then(|(x, y)| {
            let index = self.frame.bubble_at(x, y)?;
            self.shown.get(index).map(|(id, _)| *id)
        });
        if hovered != self.hovered {
            self.hovered = hovered;
            self.stack.hover(hovered, Instant::now());
            // The frame changes color under the pointer.
            self.dirty = true;
        }
    }

    /// Which notification and which part of it is at a point on the surface.
    fn part_at(&self, x: i32, y: i32) -> Option<(u32, Part)> {
        let index = self.frame.bubble_at(x, y)?;
        let rect = self.frame.bubbles[index];
        let (id, bubble) = self.shown.get(index)?;
        let part = if bubble.chrome.button.contains(x - rect.x, y - rect.y) {
            Part::Button
        } else {
            Part::Body
        };
        Some((*id, part))
    }

    /// A press and release on the same part of the same notification.
    fn click(&mut self, qh: &QueueHandle<Self>, id: u32, part: Part, button: u32) {
        match (button, part) {
            (BTN_LEFT, Part::Body) => {
                let Some(notification) = self.stack.get(id) else {
                    return;
                };
                let resident = notification.hints.resident;
                if let Some(action) = notification.default_action() {
                    let key = action.key.clone();
                    self.bus.action_invoked(id, &key);
                    // A resident notification stays after its action; the specification leaves
                    // closing it to the sender.
                    if !resident {
                        self.dismiss(id);
                    }
                } else {
                    self.dismiss(id);
                }
            }
            // The window-menu button, or a right click anywhere on it.
            (BTN_LEFT, Part::Button) | (BTN_RIGHT, _) => self.dismiss(id),
            _ => return,
        }
        self.refresh(qh);
    }

    fn draw_if_dirty(&mut self) {
        if !self.dirty || !self.configured || self.layer.is_none() || self.shown.is_empty() {
            return;
        }
        self.dirty = false;
        if self.draw() {
            // On screen now, so their clocks start. Only the ones the frame actually holds: a
            // notification cut off by a short screen has not been seen.
            self.stack.start_clocks(Instant::now(), self.shown.len());
        }
    }

    /// Paint the stack. Returns whether a frame was committed.
    fn draw(&mut self) -> bool {
        let Osd {
            layer,
            pool,
            fonts,
            palette,
            frame,
            shown,
            looks,
            stack,
            hovered,
            pressed,
            width,
            height,
            compositor,
            ..
        } = self;
        let Some(layer) = layer.as_ref() else {
            return false;
        };
        let (width, height) = ((*width).max(1) as i32, (*height).max(1) as i32);
        let (buffer, pixels) =
            match pool.create_buffer(width, height, width * 4, wl_shm::Format::Argb8888) {
                Ok(pair) => pair,
                Err(err) => {
                    eprintln!("wlrix-osd: could not get a buffer to draw into: {err}");
                    return false;
                }
            };

        let scene_shown: Vec<paint::Shown> = shown
            .iter()
            .map(|(id, bubble)| {
                let notification = stack.get(*id);
                paint::Shown {
                    bubble,
                    picture: looks.get(id).and_then(|look| look.picture.as_ref()),
                    active: *hovered == Some(*id)
                        || notification.is_some_and(|n| n.hints.urgency == Urgency::Critical),
                    pressed: *pressed == Some((*id, Part::Button, BTN_LEFT)),
                    value: notification.and_then(|n| n.hints.value),
                }
            })
            .collect();
        let waiting = stack.len() - shown.len().min(stack.len());

        let mut canvas = Canvas::new(pixels, width, height);
        paint::stack(
            &mut canvas,
            &mut paint::Scene {
                palette,
                fonts,
                frame,
                shown: &scene_shown,
                waiting,
            },
        );

        let surface = layer.wl_surface();
        // Only the notifications themselves take input; the gaps between them belong to the
        // window underneath. Double-buffered state, applied by the commit below.
        match Region::new(&*compositor) {
            Ok(region) => {
                for rect in frame.bubbles.iter().chain(frame.more.iter()) {
                    region.add(rect.x, rect.y, rect.w, rect.h);
                }
                surface.set_input_region(Some(region.wl_region()));
            }
            Err(err) => eprintln!("wlrix-osd: could not set the input region: {err}"),
        }
        surface.damage_buffer(0, 0, width, height);
        if let Err(err) = buffer.attach_to(surface) {
            eprintln!("wlrix-osd: could not attach the buffer: {err}");
            return false;
        }
        surface.commit();
        true
    }
}

/// The font, as [`crate::layout`] wants to see it.
struct Measured<'a> {
    fonts: &'a mut Fonts,
}

impl Text for Measured<'_> {
    fn width(&mut self, face: Face, px: f32, text: &str) -> i32 {
        self.fonts.width(face, px, text)
    }

    fn line_height(&mut self, face: Face, px: f32) -> i32 {
        self.fonts.line_height(face, px)
    }
}

/// The layer-shell anchor bits for a corner.
fn shell_anchor(anchor: Anchor) -> ShellAnchor {
    match anchor {
        Anchor::TopRight => ShellAnchor::TOP | ShellAnchor::RIGHT,
        Anchor::TopLeft => ShellAnchor::TOP | ShellAnchor::LEFT,
        Anchor::BottomRight => ShellAnchor::BOTTOM | ShellAnchor::RIGHT,
        Anchor::BottomLeft => ShellAnchor::BOTTOM | ShellAnchor::LEFT,
    }
}

/// Hold the stack `margin` away from the two edges it is anchored to.
fn set_margin(layer: &LayerSurface, anchor: Anchor, margin: i32) {
    let (top, bottom) = if anchor.is_top() {
        (margin, 0)
    } else {
        (0, margin)
    };
    let (left, right) = if anchor.is_left() {
        (margin, 0)
    } else {
        (0, margin)
    };
    layer.set_margin(top, right, bottom, left);
}

impl CompositorHandler for Osd {
    fn scale_factor_changed(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _s: &WlSurface,
        _f: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _s: &WlSurface,
        _t: wayland_client::protocol::wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _c: &Connection, _q: &QueueHandle<Self>, _s: &WlSurface, _t: u32) {}
    fn surface_enter(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _s: &WlSurface,
        _o: &WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _s: &WlSurface,
        _o: &WlOutput,
    ) {
    }
}

impl OutputHandler for Osd {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _c: &Connection, qh: &QueueHandle<Self>, _o: WlOutput) {
        // The way back from every monitor going at once: the compositor destroys the outputs on
        // a blank and advertises them again on wake.
        self.refresh(qh);
    }

    fn update_output(&mut self, _c: &Connection, qh: &QueueHandle<Self>, output: WlOutput) {
        if self.output.as_ref() != Some(&output) {
            return;
        }
        // A resolution change moves how tall the stack may grow.
        self.output_size = self
            .output_state
            .info(&output)
            .and_then(|info| info.logical_size);
        self.refresh(qh);
    }

    fn output_destroyed(&mut self, _c: &Connection, qh: &QueueHandle<Self>, output: WlOutput) {
        if self.output.as_ref() != Some(&output) {
            return;
        }
        // The departing output has to be named: sctk calls this before taking it out of its
        // list, and a plain refresh can pick it again -- the tray's invisible-after-blank bug.
        self.forget_surface();
        self.refresh_avoiding(qh, Some(&output));
    }
}

impl LayerShellHandler for Osd {
    fn closed(&mut self, _c: &Connection, qh: &QueueHandle<Self>, layer: &LayerSurface) {
        // Not the end of the daemon: the compositor closes every layer surface on an output it is
        // removing, which is what a monitor entering power save looks like.
        if self.layer.as_ref().map(WaylandSurface::wl_surface) != Some(layer.wl_surface()) {
            return;
        }
        self.forget_surface();
        self.refresh(qh);
    }

    fn configure(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        self.configured = true;
        let (width, height) = match configure.new_size {
            (0, 0) => self.requested,
            size => size,
        };
        self.width = width;
        self.height = height;
        self.dirty = true;
    }
}

impl SeatHandler for Osd {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _c: &Connection, _q: &QueueHandle<Self>, _seat: WlSeat) {}

    fn new_capability(
        &mut self,
        _c: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            match self.seat_state.get_pointer(qh, &seat) {
                Ok(pointer) => self.pointer = Some(pointer),
                // Notifications still appear and expire; they just cannot be clicked.
                Err(err) => eprintln!("wlrix-osd: no pointer: {err}"),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _c: &Connection,
        _q: &QueueHandle<Self>,
        _seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer
            && let Some(pointer) = self.pointer.take()
        {
            pointer.release();
        }
    }

    fn remove_seat(&mut self, _c: &Connection, _q: &QueueHandle<Self>, _seat: WlSeat) {}
}

impl PointerHandler for Osd {
    fn pointer_frame(
        &mut self,
        _c: &Connection,
        qh: &QueueHandle<Self>,
        _pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        let Some(surface) = self.layer.as_ref().map(|layer| layer.wl_surface().clone()) else {
            return;
        };
        for event in events {
            if event.surface != surface {
                continue;
            }
            let (x, y) = (event.position.0 as i32, event.position.1 as i32);
            match event.kind {
                PointerEventKind::Enter { .. } | PointerEventKind::Motion { .. } => {
                    self.pointer_at = Some((x, y));
                    self.update_hover();
                }
                PointerEventKind::Leave { .. } => {
                    self.pointer_at = None;
                    self.dirty |= self.pressed.take().is_some();
                    self.update_hover();
                }
                PointerEventKind::Press { button, .. } => {
                    self.pressed = self.part_at(x, y).map(|(id, part)| (id, part, button));
                    self.dirty = true;
                }
                PointerEventKind::Release { button, .. } => {
                    let pressed = self.pressed.take();
                    self.dirty |= pressed.is_some();
                    // Only a press and release on the same part of the same notification counts,
                    // so dragging off before letting go cancels -- as a button does.
                    if let Some((id, part, pressed_button)) = pressed
                        && pressed_button == button
                        && self.part_at(x, y) == Some((id, part))
                    {
                        self.click(qh, id, part, button);
                    }
                }
                _ => {}
            }
        }
    }
}

impl ShmHandler for Osd {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Osd {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

delegate_compositor!(Osd);
delegate_output!(Osd);
delegate_shm!(Osd);
delegate_seat!(Osd);
delegate_pointer!(Osd);
delegate_layer!(Osd);
delegate_registry!(Osd);
