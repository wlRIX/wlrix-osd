// SPDX-License-Identifier: GPL-3.0-or-later
//! The default speaker and microphone: their levels, and the keys that change them.
//!
//! # Through PipeWire's PulseAudio server
//!
//! PipeWire serves the PulseAudio protocol (`pipewire-pulse`), and through it the two things
//! this needs are one call each: set or mute `@DEFAULT_SINK@`, and subscribe to changes so the
//! bar appears whatever changed the level -- these keys, the Audio Panel, a script. PipeWire's
//! native API reaches the same place by way of the session manager's metadata object, the
//! default node's id, and SPA pods built by hand for its `Props`; that is a great deal of code to
//! be wrong in for no gain a person could hear. The Audio Panel takes the same road for the same
//! reason.
//!
//! # Threads
//!
//! libpulse's threaded mainloop runs its own thread and calls back on it. Those callbacks only
//! read what the server says and send an [`AudioEvent`] down the main loop's channel. The calls
//! that *change* something are made from the main loop, holding the mainloop's lock -- the
//! arrangement libpulse documents -- and return at once; the change comes back as an event like
//! any other.
//!
//! # Percent
//!
//! A level is shown as `pactl` and `wpctl` show it: the share of PulseAudio's normal volume,
//! where 100 % is unamplified. Past 100 is amplification, which `volume_max` may allow.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::subscribe::{Facility, InterestMaskSet};
use pulse::context::{Context, FlagSet, State};
use pulse::mainloop::threaded::Mainloop;
use pulse::volume::{ChannelVolumes, Volume};

/// What the audio side tells the main loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioEvent {
    /// The default output, as it now is.
    Output {
        percent: u32,
        muted: bool,
        /// The device's name, which changes when the default output is switched.
        name: String,
        description: String,
    },
    /// The default input's mute, as it now is.
    Input { muted: bool, description: String },
    /// The connection to the server went, and the levels with it. The loop starts again later.
    Lost,
}

/// The last levels heard, which the key handlers step from.
#[derive(Default)]
struct Levels {
    output: Option<ChannelVolumes>,
    output_muted: bool,
    input_muted: bool,
}

/// The connection to the audio server.
pub struct Audio {
    mainloop: Rc<RefCell<Mainloop>>,
    context: Rc<RefCell<Context>>,
    levels: Arc<Mutex<Levels>>,
}

/// The default output and input, as the PulseAudio protocol names them.
const DEFAULT_SINK: &str = "@DEFAULT_SINK@";
const DEFAULT_SOURCE: &str = "@DEFAULT_SOURCE@";

impl Audio {
    /// Connect, and report the levels on `events` from now on. `None` when libpulse cannot even
    /// start; a server that is not there yet is waited for (`NOFAIL`), since this daemon is
    /// started early in the session and PipeWire may still be coming up.
    pub fn start(events: calloop::channel::Sender<AudioEvent>) -> Option<Self> {
        let mainloop = Rc::new(RefCell::new(Mainloop::new()?));
        let context = Rc::new(RefCell::new(Context::new(
            &*mainloop.borrow(),
            "wlrix-osd",
        )?));
        let levels = Arc::new(Mutex::new(Levels::default()));

        let state_context = Rc::clone(&context);
        let state_levels = Arc::clone(&levels);
        let state_events = events.clone();
        context
            .borrow_mut()
            .set_state_callback(Some(Box::new(move || {
                // libpulse also calls this from inside `connect` and `disconnect`, on the calling
                // thread, while the context is still borrowed for that call. Those are
                // `Connecting` and `Terminated`, neither of which needs acting on, so a context
                // that is busy is simply left alone.
                let Ok(context) = state_context.try_borrow() else {
                    return;
                };
                let state = context.get_state();
                drop(context);
                match state {
                    State::Ready => {
                        subscribe(&state_context, &state_levels, &state_events);
                        query(&state_context, &state_levels, &state_events);
                    }
                    State::Failed | State::Terminated => {
                        let _ = state_events.send(AudioEvent::Lost);
                    }
                    _ => {}
                }
            })));
        context
            .borrow_mut()
            .connect(None, FlagSet::NOFAIL, None)
            .ok()?;
        mainloop.borrow_mut().start().ok()?;
        Some(Self {
            mainloop,
            context,
            levels,
        })
    }

    /// Run `call` with the mainloop locked, which is how libpulse may be called from outside its
    /// own thread.
    fn locked(&self, call: impl FnOnce(&mut Context)) {
        self.mainloop.borrow_mut().lock();
        call(&mut self.context.borrow_mut());
        self.mainloop.borrow_mut().unlock();
    }

    /// The output level and mute as last heard, for showing the bar after a key that changed
    /// nothing -- up, already at the top.
    pub fn output(&self) -> Option<(u32, bool)> {
        let levels = self.levels.lock().unwrap_or_else(|err| err.into_inner());
        Some((percent(levels.output.as_ref()?.avg()), levels.output_muted))
    }

    pub fn input_muted(&self) -> bool {
        self.levels
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .input_muted
    }

    /// One step up or down, returning the level and mute it is moving to so the bar can say so
    /// at once. Turning it up also unmutes, as every volume key does: a person pressing up on a
    /// muted speaker wants to hear it. `None` before the server has said what the level is.
    pub fn step(&self, up: bool, step_percent: u32, max_percent: u32) -> Option<(u32, bool)> {
        let (current, muted) = {
            let levels = self.levels.lock().unwrap_or_else(|err| err.into_inner());
            (levels.output?, levels.output_muted)
        };
        let target = stepped(current, up, step_percent, max_percent);
        self.locked(|context| {
            let mut introspect = context.introspect();
            introspect.set_sink_volume_by_name(DEFAULT_SINK, &target, None);
            if up && muted {
                introspect.set_sink_mute_by_name(DEFAULT_SINK, false, None);
            }
        });
        Some((percent(target.max()), muted && !up))
    }

    /// Mute or unmute, returning the level and the mute it is moving to.
    pub fn toggle_mute(&self) -> Option<(u32, bool)> {
        let (level, muted) = self.output()?;
        self.locked(|context| {
            context
                .introspect()
                .set_sink_mute_by_name(DEFAULT_SINK, !muted, None);
        });
        Some((level, !muted))
    }

    /// Mute or unmute the microphone, returning whether it is now muted.
    pub fn toggle_microphone(&self) -> bool {
        let muted = self.input_muted();
        self.locked(|context| {
            context
                .introspect()
                .set_source_mute_by_name(DEFAULT_SOURCE, !muted, None);
        });
        !muted
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.mainloop.borrow_mut().lock();
        // Detached first: they hold the context, and disconnecting calls them.
        self.context.borrow_mut().set_state_callback(None);
        self.context.borrow_mut().set_subscribe_callback(None);
        self.context.borrow_mut().disconnect();
        self.mainloop.borrow_mut().unlock();
        self.mainloop.borrow_mut().stop();
    }
}

/// Ask to hear about changes to outputs, inputs, and which of them are the defaults.
fn subscribe(
    context: &Rc<RefCell<Context>>,
    levels: &Arc<Mutex<Levels>>,
    events: &calloop::channel::Sender<AudioEvent>,
) {
    let callback_context = Rc::clone(context);
    let callback_levels = Arc::clone(levels);
    let callback_events = events.clone();
    let mut context_ref = context.borrow_mut();
    context_ref.set_subscribe_callback(Some(Box::new(move |facility, _operation, _index| {
        // Any change to any output or input, or to the server's idea of the defaults, means read
        // the two defaults again. Cheaper than working out whether the change was to one of them.
        if matches!(
            facility,
            Some(Facility::Sink | Facility::Source | Facility::Server)
        ) {
            query(&callback_context, &callback_levels, &callback_events);
        }
    })));
    context_ref.subscribe(
        InterestMaskSet::SINK | InterestMaskSet::SOURCE | InterestMaskSet::SERVER,
        |_| {},
    );
}

/// Read the default output and input, and report them.
fn query(
    context: &Rc<RefCell<Context>>,
    levels: &Arc<Mutex<Levels>>,
    events: &calloop::channel::Sender<AudioEvent>,
) {
    let introspect = context.borrow().introspect();
    let output_levels = Arc::clone(levels);
    let output_events = events.clone();
    introspect.get_sink_info_by_name(DEFAULT_SINK, move |result| {
        if let ListResult::Item(info) = result {
            {
                let mut levels = output_levels.lock().unwrap_or_else(|err| err.into_inner());
                levels.output = Some(info.volume);
                levels.output_muted = info.mute;
            }
            let _ = output_events.send(AudioEvent::Output {
                percent: percent(info.volume.avg()),
                muted: info.mute,
                name: info.name.as_deref().unwrap_or_default().to_owned(),
                description: info.description.as_deref().unwrap_or_default().to_owned(),
            });
        }
    });
    let input_levels = Arc::clone(levels);
    let input_events = events.clone();
    introspect.get_source_info_by_name(DEFAULT_SOURCE, move |result| {
        if let ListResult::Item(info) = result {
            input_levels
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .input_muted = info.mute;
            let _ = input_events.send(AudioEvent::Input {
                muted: info.mute,
                description: info.description.as_deref().unwrap_or_default().to_owned(),
            });
        }
    });
}

/// A volume as a percentage of normal, rounded to the nearest whole one.
pub fn percent(volume: Volume) -> u32 {
    ((volume.0 as u64 * 100 + Volume::NORMAL.0 as u64 / 2) / Volume::NORMAL.0 as u64) as u32
}

/// A percentage of normal as a volume.
fn from_percent(percent: u32) -> Volume {
    Volume((percent as u64 * Volume::NORMAL.0 as u64 / 100) as u32)
}

/// `current`, one step up or down, every channel moved by the same amount so the balance is
/// kept. Up stops at `max_percent`; down stops at silence.
///
/// A step lands on a multiple of itself rather than adding to wherever the level was: a level
/// of 47 % goes to 50, then 55, rather than 52, 57 -- the numbers a person sees stay round.
pub fn stepped(
    current: ChannelVolumes,
    up: bool,
    step_percent: u32,
    max_percent: u32,
) -> ChannelVolumes {
    let step = step_percent.max(1);
    let now = percent(current.max());
    let target = if up {
        ((now / step + 1) * step).min(max_percent.max(step))
    } else {
        (now.saturating_sub(1) / step) * step
    };
    let mut volumes = current;
    if current.max().0 == 0 {
        // Silent on every channel: scaling keeps proportions, and zero has none to keep.
        volumes.set(current.len(), from_percent(target));
    } else {
        volumes.scale(from_percent(target));
    }
    volumes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stereo(percent: u32) -> ChannelVolumes {
        let mut volumes = ChannelVolumes::default();
        volumes.set(2, from_percent(percent));
        volumes
    }

    fn level(volumes: &ChannelVolumes) -> u32 {
        percent(volumes.max())
    }

    #[test]
    fn percent_is_pactls() {
        assert_eq!(percent(Volume::NORMAL), 100);
        assert_eq!(percent(Volume::MUTED), 0);
        assert_eq!(percent(from_percent(37)), 37);
    }

    #[test]
    fn steps_land_on_round_numbers() {
        assert_eq!(level(&stepped(stereo(47), true, 5, 100)), 50);
        assert_eq!(level(&stepped(stereo(50), true, 5, 100)), 55);
        assert_eq!(level(&stepped(stereo(47), false, 5, 100)), 45);
        assert_eq!(level(&stepped(stereo(45), false, 5, 100)), 40);
    }

    #[test]
    fn up_stops_at_the_ceiling_and_down_at_silence() {
        assert_eq!(level(&stepped(stereo(98), true, 5, 100)), 100);
        assert_eq!(level(&stepped(stereo(100), true, 5, 100)), 100);
        assert_eq!(
            level(&stepped(stereo(100), true, 5, 150)),
            105,
            "amplification allowed"
        );
        assert_eq!(level(&stepped(stereo(3), false, 5, 100)), 0);
        assert_eq!(level(&stepped(stereo(0), false, 5, 100)), 0);
        assert_eq!(
            level(&stepped(stereo(0), true, 5, 100)),
            5,
            "up from silence"
        );
    }

    #[test]
    fn the_balance_is_kept() {
        let mut uneven = ChannelVolumes::default();
        uneven.set(2, from_percent(40));
        uneven.get_mut()[1] = from_percent(20);
        let louder = stepped(uneven, true, 5, 100);
        assert_eq!(percent(louder.get()[0]), 45);
        // The quieter channel keeps its proportion: 20/40 of 45.
        assert!((21..=23).contains(&percent(louder.get()[1])));
    }
}
