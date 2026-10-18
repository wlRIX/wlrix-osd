// SPDX-License-Identifier: GPL-3.0-or-later
//! The hand-edited settings file.
//!
//! ```toml
//! # ~/.config/wlrix/osd.toml
//! output = "DP-1"              # which monitor; default is the leftmost
//! anchor = "top-right"         # top-right | top-left | bottom-right | bottom-left
//! max_visible = 5              # notifications on screen at once; the rest wait their turn
//! width = 360                  # one notification, frame included
//!
//! [timeout]                    # seconds; 0 = until dismissed
//! low = 5
//! normal = 8
//! critical = 0
//!
//! [appearance]
//! palette = "gotham"           # the color scheme; default is "classic"
//! icon_theme = "wlrix"         # where an app_icon name is looked up; "" for none
//!
//! [metrics]
//! icon = 48                    # the picture beside the text, square
//! gap = 6                      # between notifications
//! margin = 12                  # between the stack and the screen edges
//! body_lines = 6               # the body is cut off, with an ellipsis, past this
//! ```
//!
//! Read from the user's config directory first, then `/etc/wlrix`; the first file found wins
//! outright rather than merging, and unknown keys are an error -- the same shape as every other
//! wlRIX component's file, for the same reasons.

use std::time::Duration;

use serde::Deserialize;

use crate::layout::Anchor;
use crate::notification::Urgency;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Which monitor notifications appear on, by connector name. Absent means the one at the
    /// origin, which is the tray's rule too.
    #[serde(default)]
    pub output: Option<String>,
    /// Which corner the stack grows from. Top right by default, which is where every other
    /// desktop puts it and so where a person looks.
    #[serde(default)]
    pub anchor: Anchor,
    #[serde(default)]
    max_visible: Option<usize>,
    #[serde(default)]
    width: Option<i32>,
    #[serde(default)]
    pub timeout: TimeoutConfig,
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub metrics: MetricsConfig,
}

/// How long a notification stays up when its sender left the choice to the server.
///
/// In whole seconds, because that is the unit a person thinks in when editing a file; the bus
/// says milliseconds, and [`Config::expiry`] is where the two meet.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeoutConfig {
    pub low: Option<u32>,
    pub normal: Option<u32>,
    pub critical: Option<u32>,
}

/// Which color scheme to draw in and which icon theme to look names up in. The same section, and
/// the same two keys, as the tray's.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppearanceConfig {
    /// A scheme id from `wlrix-ui`. Absent, empty, or unrecognized means the default, with a line
    /// in the log for the last of those.
    #[serde(default)]
    pub palette: Option<String>,
    /// The icon theme an `app_icon` name is looked up in, before hicolor and `/usr/share/pixmaps`.
    /// Defaults to wlrix, which inherits Adwaita -- see the tray's `config.rs` for why a named
    /// default earns its keep.
    #[serde(default)]
    pub icon_theme: Option<String>,
}

/// What [`AppearanceConfig::icon_theme`] means when the file does not say.
const DEFAULT_ICON_THEME: &str = "wlrix";

impl AppearanceConfig {
    /// The icon theme to search first.
    pub fn icon_theme(&self) -> &str {
        self.icon_theme.as_deref().unwrap_or(DEFAULT_ICON_THEME)
    }
}

/// Geometry inside and around a notification. Every field falls back to [`Metrics::default`].
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricsConfig {
    pub icon: Option<i32>,
    pub gap: Option<i32>,
    pub margin: Option<i32>,
    pub body_lines: Option<usize>,
}

/// [`MetricsConfig`], resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    pub width: i32,
    pub icon: i32,
    pub gap: i32,
    pub margin: i32,
    pub body_lines: usize,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            width: 360,
            icon: 48,
            gap: 6,
            margin: 12,
            body_lines: 6,
        }
    }
}

/// The narrowest notification that still has room for a title and a column of text beside its
/// picture. Anything less is clamped up to it rather than refused.
const MIN_WIDTH: i32 = 200;

/// The defaults [`Config::expiry`] falls back to, in seconds.
const DEFAULT_LOW: u32 = 5;
const DEFAULT_NORMAL: u32 = 8;
/// Critical notifications stay until dismissed. The specification says they "should not
/// automatically expire", and a battery about to die is not something to miss because the
/// person was looking away for eight seconds.
const DEFAULT_CRITICAL: u32 = 0;

/// How many notifications are on screen at once when the file does not say.
const DEFAULT_MAX_VISIBLE: usize = 5;

impl Config {
    /// The geometry, with anything unset left at the default and anything silly clamped.
    pub fn metrics(&self) -> Metrics {
        let base = Metrics::default();
        Metrics {
            width: self.width.unwrap_or(base.width).max(MIN_WIDTH),
            icon: self.metrics.icon.unwrap_or(base.icon).clamp(16, 128),
            gap: self.metrics.gap.unwrap_or(base.gap).max(0),
            margin: self.metrics.margin.unwrap_or(base.margin).max(0),
            body_lines: self.metrics.body_lines.unwrap_or(base.body_lines).max(1),
        }
    }

    /// How many notifications are drawn at once. At least one: a stack of zero would hold every
    /// notification back forever and say nothing about it.
    pub fn max_visible(&self) -> usize {
        self.max_visible.unwrap_or(DEFAULT_MAX_VISIBLE).max(1)
    }

    /// How long a notification lasts once it is on screen. `None` is "until dismissed".
    ///
    /// `requested` is the sender's `expire_timeout`, in milliseconds: `-1` leaves it to the
    /// server, `0` asks for no expiry at all, anything else is honored. The one exception is
    /// urgency: a critical notification uses the configured value whatever its sender asked
    /// for, since the specification asks for critical notifications not to expire and a sender
    /// passing a stock five seconds rarely meant to override that.
    pub fn expiry(&self, urgency: Urgency, requested: i32) -> Option<Duration> {
        let seconds = |configured: Option<u32>, default: u32| {
            let seconds = configured.unwrap_or(default);
            (seconds > 0).then(|| Duration::from_secs(seconds.into()))
        };
        match (urgency, requested) {
            (Urgency::Critical, _) => seconds(self.timeout.critical, DEFAULT_CRITICAL),
            (_, 0) => None,
            (_, ms) if ms > 0 => Some(Duration::from_millis(ms as u64)),
            (Urgency::Low, _) => seconds(self.timeout.low, DEFAULT_LOW),
            (Urgency::Normal, _) => seconds(self.timeout.normal, DEFAULT_NORMAL),
        }
    }

    /// Load the first config file that exists. No file at all is not an error -- the defaults are
    /// a working notification daemon.
    pub fn load() -> Self {
        for path in crate::xdg::config_paths() {
            let text = match std::fs::read_to_string(&path) {
                Ok(text) => text,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => {
                    eprintln!("wlrix-osd: could not read {}: {err}", path.display());
                    continue;
                }
            };
            match toml::from_str::<Self>(&text) {
                Ok(config) => return config,
                Err(err) => {
                    // Loud, then carry on with defaults: a typo in a config file must not cost
                    // the session its notifications.
                    eprintln!("wlrix-osd: {} is not valid: {err}", path.display());
                    return Self::default();
                }
            }
        }
        Self::default()
    }
}

/// Parse a candidate config file, for `--check-config`.
///
/// `wlrix-settings-daemon` writes a temporary file and runs this against it before renaming it
/// into place, so a settings panel cannot produce a file this program would refuse. Deliberately
/// not [`Config::load`], which carries on with defaults where this has to say no.
pub fn check(path: &std::path::Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("could not read {}: {err}", path.display()))?;
    toml::from_str::<Config>(&text)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Config {
        toml::from_str(text).expect("the test config should parse")
    }

    #[test]
    fn an_empty_file_is_all_defaults() {
        let config = config("");
        assert_eq!(config.output, None);
        assert_eq!(config.anchor, Anchor::TopRight);
        assert_eq!(config.max_visible(), 5);
        assert_eq!(config.metrics(), Metrics::default());
        assert_eq!(config.appearance.icon_theme(), "wlrix");
    }

    #[test]
    fn a_typo_is_refused_rather_than_ignored() {
        assert!(toml::from_str::<Config>("max_visable = 3\n").is_err());
        assert!(toml::from_str::<Config>("[timeout]\nnormall = 3\n").is_err());
        assert!(toml::from_str::<Config>("anchor = \"top right\"\n").is_err());
    }

    #[test]
    fn the_server_default_depends_on_urgency() {
        let config = config("");
        assert_eq!(
            config.expiry(Urgency::Low, -1),
            Some(Duration::from_secs(5))
        );
        assert_eq!(
            config.expiry(Urgency::Normal, -1),
            Some(Duration::from_secs(8))
        );
        assert_eq!(config.expiry(Urgency::Critical, -1), None);
    }

    #[test]
    fn a_sender_may_choose_unless_it_is_critical() {
        let config = config("");
        assert_eq!(
            config.expiry(Urgency::Normal, 2500),
            Some(Duration::from_millis(2500))
        );
        assert_eq!(config.expiry(Urgency::Normal, 0), None, "0 means never");
        // A critical notification sent with a stock timeout still stays up.
        assert_eq!(config.expiry(Urgency::Critical, 5000), None);
        // ...unless the person said otherwise.
        let impatient = super::tests::config("[timeout]\ncritical = 30\n");
        assert_eq!(
            impatient.expiry(Urgency::Critical, 5000),
            Some(Duration::from_secs(30))
        );
    }

    #[test]
    fn a_configured_zero_means_until_dismissed() {
        let config = config("[timeout]\nnormal = 0\n");
        assert_eq!(config.expiry(Urgency::Normal, -1), None);
    }

    #[test]
    fn nonsense_geometry_is_clamped_not_fatal() {
        let config = config(
            "width = 10\nmax_visible = 0\n[metrics]\nicon = 2000\ngap = -4\nmargin = -1\n\
             body_lines = 0\n",
        );
        let metrics = config.metrics();
        assert_eq!(metrics.width, MIN_WIDTH);
        assert_eq!(metrics.icon, 128);
        assert_eq!(metrics.gap, 0);
        assert_eq!(metrics.margin, 0);
        assert_eq!(metrics.body_lines, 1);
        assert_eq!(config.max_visible(), 1);
    }

    #[test]
    fn a_full_file_round_trips() {
        let config = config(
            "output = \"DP-1\"\nanchor = \"bottom-left\"\nmax_visible = 3\nwidth = 400\n\
             [timeout]\nlow = 2\nnormal = 4\ncritical = 60\n\
             [appearance]\npalette = \"gotham\"\nicon_theme = \"\"\n\
             [metrics]\nicon = 32\ngap = 2\nmargin = 20\nbody_lines = 3\n",
        );
        assert_eq!(config.output.as_deref(), Some("DP-1"));
        assert_eq!(config.anchor, Anchor::BottomLeft);
        assert_eq!(config.max_visible(), 3);
        assert_eq!(config.appearance.palette.as_deref(), Some("gotham"));
        assert_eq!(config.appearance.icon_theme(), "");
        assert_eq!(
            config.metrics(),
            Metrics {
                width: 400,
                icon: 32,
                gap: 2,
                margin: 20,
                body_lines: 3,
            }
        );
        assert_eq!(
            config.expiry(Urgency::Low, -1),
            Some(Duration::from_secs(2))
        );
    }
}
