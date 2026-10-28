// SPDX-License-Identifier: GPL-3.0-or-later
//! wlRIX's notification daemon and on-screen display.
//!
//! This program is `org.freedesktop.Notifications`: what `notify-send`, a browser's "download
//! complete" and a chat client's new message all go to. Each notification is drawn as a small
//! fixed-size 4Dwm window, stacked in a corner of the screen on the wlr-layer-shell overlay layer.
//!
//! Two halves, kept apart on purpose, the tray's split:
//!
//! - [`dbus`] is the bus. It owns the name, answers `Notify`, and converts what arrives into a
//!   [`notification::Notification`] on zbus's own thread.
//! - [`ui`] is the window: one layer surface, exactly as big as the stack it draws.
//!
//! Between them sit the parts that are neither, and are testable without a bus or a screen:
//! [`markup`] (the body's tags), [`stack`] (order, replacement and expiry), [`layout`] (where
//! everything goes), [`pixmap`] and [`icons`] (getting from what a sender named to pixels).
//!
//! The on-screen bar is the other half: [`control`] takes the volume and brightness keys from the
//! compositor, [`volume`] and [`brightness`] change the levels, and [`bar`] says what changed.

pub mod bar;
pub mod brightness;
pub mod config;
pub mod control;
pub mod dbus;
pub mod i18n;
pub mod icons;
pub mod layout;
pub mod markup;
pub mod notification;
pub mod pidfile;
pub mod pixmap;
pub mod signals;
pub mod sound;
pub mod stack;
pub mod ui;
pub mod volume;
pub mod xdg;
