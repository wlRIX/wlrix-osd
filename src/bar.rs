// SPDX-License-Identifier: GPL-3.0-or-later
//! What the on-screen bar says.
//!
//! One small panel, low in the middle of the screen, saying what a volume or brightness key just
//! did: an icon, a line of text, and a bar for the level. It replaces itself on every change and
//! is gone a moment after the last -- never stacked, never in the way. It takes no input at all,
//! so it is not even in the way of the pointer.
//!
//! This module is only the words and the icon, so they are testable; [`crate::ui`] draws them.
//!
//! The icons are the freedesktop symbolic names every desktop's OSD uses, found through the
//! configured icon theme -- the `wlrix` theme inherits Adwaita, which has them all.

use crate::i18n::tr;

/// One bar's contents.
///
/// Laid out as IRIX laid out a level: the number at the top left, over the bar, with no percent
/// sign. What is being measured goes at the top right, as the caption; a bar with no level --
/// the microphone, a missing backlight -- has only the caption, as one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bar {
    pub icon: &'static str,
    /// What is measured ("Volume", a device's name), or the whole message when there is no level.
    pub label: String,
    /// What goes at the top left: the level as a bare number, or a word in its place ("Muted").
    /// `None` exactly when there is no level.
    pub number: Option<String>,
    /// The level, `0..=max`. `None` draws no bar: a microphone is muted or it is not.
    pub value: Option<u32>,
    /// What a full bar means. 100, or more when amplification is allowed, so 120 % reads as
    /// past the end of a normal bar rather than as full.
    pub max: u32,
}

impl Bar {
    /// The speaker. `device` names it, when the default output has just changed.
    pub fn volume(percent: u32, muted: bool, max_percent: u32, device: Option<&str>) -> Self {
        let icon = if muted || percent == 0 {
            "audio-volume-muted-symbolic"
        } else if percent > 100 {
            "audio-volume-overamplified-symbolic"
        } else if percent >= 67 {
            "audio-volume-high-symbolic"
        } else if percent >= 34 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-low-symbolic"
        };
        let label = match device {
            Some(device) if !device.is_empty() => device.to_owned(),
            _ => tr("Volume", "音量").to_owned(),
        };
        Self {
            icon,
            label,
            number: Some(if muted {
                tr("Muted", "ミュート").to_owned()
            } else {
                percent.to_string()
            }),
            // A muted bar is drawn empty rather than left out, so the level comes back where it
            // was shown going away.
            value: Some(if muted { 0 } else { percent }),
            max: max_percent.max(100),
        }
    }

    pub fn microphone(muted: bool) -> Self {
        Self {
            icon: if muted {
                "microphone-sensitivity-muted-symbolic"
            } else {
                "microphone-sensitivity-high-symbolic"
            },
            label: if muted {
                tr("Microphone muted", "マイクはミュート中").to_owned()
            } else {
                tr("Microphone on", "マイクはオン").to_owned()
            },
            number: None,
            value: None,
            max: 100,
        }
    }

    /// The backlight, or the absence of one.
    pub fn brightness(percent: Option<u32>) -> Self {
        match percent {
            Some(percent) => Self {
                icon: "display-brightness-symbolic",
                label: tr("Brightness", "明るさ").to_owned(),
                number: Some(percent.to_string()),
                value: Some(percent),
                max: 100,
            },
            None => Self {
                icon: "display-brightness-symbolic",
                label: tr(
                    "No backlight to adjust",
                    "調整できるバックライトがありません",
                )
                .to_owned(),
                number: None,
                value: None,
                max: 100,
            },
        }
    }

    /// Anything another program asks to show through `com.wlrix.OSD.Show`. A value past 100 is
    /// drawn full; a negative one means no bar.
    pub fn other(icon: &str, label: &str, value: i32) -> (String, Self) {
        let value = (value >= 0).then(|| value.min(100) as u32);
        (
            icon.to_owned(),
            Self {
                icon: "",
                label: label.to_owned(),
                number: value.map(|value| value.to_string()),
                value,
                max: 100,
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_volume_icon_follows_the_level() {
        assert_eq!(
            Bar::volume(10, false, 100, None).icon,
            "audio-volume-low-symbolic"
        );
        assert_eq!(
            Bar::volume(50, false, 100, None).icon,
            "audio-volume-medium-symbolic"
        );
        assert_eq!(
            Bar::volume(90, false, 100, None).icon,
            "audio-volume-high-symbolic"
        );
        assert_eq!(
            Bar::volume(0, false, 100, None).icon,
            "audio-volume-muted-symbolic"
        );
        assert_eq!(
            Bar::volume(120, false, 150, None).icon,
            "audio-volume-overamplified-symbolic"
        );
    }

    #[test]
    fn the_level_is_a_bare_number_and_the_caption_says_what_of() {
        let bar = Bar::volume(35, false, 100, None);
        assert_eq!(bar.number.as_deref(), Some("35"));
        assert_eq!(bar.label, "Volume");
        assert_eq!(Bar::brightness(Some(40)).number.as_deref(), Some("40"));
    }

    #[test]
    fn muted_says_so_and_empties_the_bar() {
        let bar = Bar::volume(70, true, 100, None);
        assert_eq!(bar.number.as_deref(), Some("Muted"));
        assert_eq!(bar.value, Some(0));
        assert_eq!(bar.icon, "audio-volume-muted-symbolic");
    }

    #[test]
    fn a_new_default_output_is_named() {
        let bar = Bar::volume(40, false, 100, Some("Sound Blaster X5"));
        assert_eq!(bar.label, "Sound Blaster X5");
        assert_eq!(bar.number.as_deref(), Some("40"));
    }

    #[test]
    fn amplification_widens_the_scale() {
        assert_eq!(Bar::volume(120, false, 150, None).max, 150);
        assert_eq!(Bar::volume(50, false, 100, None).max, 100);
    }

    #[test]
    fn the_microphone_and_a_missing_backlight_have_no_bar() {
        assert_eq!(Bar::microphone(true).value, None);
        assert_eq!(Bar::microphone(true).label, "Microphone muted");
        assert_eq!(Bar::brightness(None).value, None);
        assert_eq!(Bar::brightness(None).number, None);
    }

    #[test]
    fn another_programs_bar_is_clamped() {
        let (icon, bar) = Bar::other("network-wireless-disabled-symbolic", "Wireless off", -1);
        assert_eq!(icon, "network-wireless-disabled-symbolic");
        assert_eq!(bar.value, None);
        assert_eq!(Bar::other("x", "y", 250).1.value, Some(100));
    }
}
