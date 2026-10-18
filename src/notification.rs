// SPDX-License-Identifier: GPL-3.0-or-later
//! One notification, as the rest of the program sees it.
//!
//! What arrives over the bus is eight loosely typed arguments and a dictionary of variants. By
//! the time it reaches the main loop it is this: markup already parsed, hints already read
//! into fields, actions already paired up, and nothing left that can fail to convert. The bus
//! thread does the converting ([`crate::dbus`]), so the loop only ever handles values it can
//! trust the shape of.

use std::collections::HashMap;

use zbus::zvariant::{OwnedValue, Value};

use crate::markup::Span;
use crate::pixmap::Pixmap;

/// The specification's three urgency levels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Urgency {
    Low,
    #[default]
    Normal,
    Critical,
}

impl Urgency {
    /// The hint's byte. Anything past 2 is read as critical rather than refused: a sender that
    /// means "more urgent than critical" still means critical.
    fn from_level(level: i64) -> Self {
        match level {
            i64::MIN..=0 => Self::Low,
            1 => Self::Normal,
            _ => Self::Critical,
        }
    }
}

/// One button the sender offered, by the key it wants back and the label it wants shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub key: String,
    pub label: String,
}

/// The action the specification reserves for clicking the notification itself.
pub const DEFAULT_ACTION: &str = "default";

/// Why a notification went away, as `NotificationClosed` reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Expired = 1,
    Dismissed = 2,
    /// `CloseNotification`.
    Closed = 3,
    Undefined = 4,
}

/// The hints this server acts on. Everything else in the dictionary is ignored, which is what
/// the specification asks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hints {
    pub urgency: Urgency,
    pub category: Option<String>,
    /// The sender's `.desktop` file, without the extension. Where a name and an icon come from
    /// when the sender gave neither.
    pub desktop_entry: Option<String>,
    /// `image-data`, decoded. The specification's first choice of picture.
    pub image: Option<Pixmap>,
    /// `image-path`: a file path, a `file://` URI, or an icon name.
    pub image_path: Option<String>,
    /// `icon_data`, from version 1.0 of the specification. Last resort.
    pub icon_data: Option<Pixmap>,
    /// `resident`: the notification stays after an action is invoked.
    pub resident: bool,
    /// `transient`: never kept in history. Read now so the history to come does not have to
    /// change what the bus thread hands over.
    pub transient: bool,
    /// `value`, a percentage for a progress bar.
    pub value: Option<i32>,
    /// `x-canonical-private-synchronous` or `x-dunst-stack-tag`: a sender's own name for a
    /// notification it keeps replacing, such as a volume level. A new one with the same tag,
    /// from the same application, takes the old one's place instead of stacking under it.
    pub stack_tag: Option<String>,
}

/// A notification ready to show.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub app_icon: String,
    /// Plain text. The specification allows no markup in the summary.
    pub summary: String,
    pub body: Vec<Span>,
    pub actions: Vec<Action>,
    pub hints: Hints,
    /// The sender's `expire_timeout`, in milliseconds, as sent. [`crate::config::Config::expiry`]
    /// turns it into a duration once urgency is known.
    pub expire_timeout: i32,
}

impl Notification {
    /// The action clicking the notification runs, if the sender offered one.
    pub fn default_action(&self) -> Option<&Action> {
        self.actions
            .iter()
            .find(|action| action.key == DEFAULT_ACTION)
    }
}

/// Pair up the flat `[key, label, key, label, …]` list the bus carries.
///
/// An odd one out at the end has no label and is dropped. A key with an empty label is kept:
/// `default` is commonly sent that way, since it is never drawn as a button.
pub fn actions(flat: &[String]) -> Vec<Action> {
    flat.as_chunks::<2>()
        .0
        .iter()
        .map(|[key, label]| Action {
            key: key.clone(),
            label: label.clone(),
        })
        .collect()
}

impl Hints {
    /// Read the hints dictionary.
    ///
    /// Forgiving about types the way senders are careless about them: `urgency` is a byte by
    /// the specification and an `int32` from half the scripts that set it, and both work. A
    /// hint whose value cannot be read as anything sensible is skipped, never an error -- one
    /// malformed hint must not cost the notification.
    ///
    /// The specification spells two of these with underscores in its older versions
    /// (`image_data`, `image_path`), and both spellings are still sent.
    pub fn read(hints: &HashMap<String, OwnedValue>) -> Self {
        let get = |names: &[&str]| names.iter().find_map(|name| hints.get(*name));
        let string = |names: &[&str]| get(names).and_then(|value| as_string(value));
        let flag = |name: &str| get(&[name]).and_then(|value| as_bool(value)) == Some(true);

        Self {
            urgency: get(&["urgency"])
                .and_then(|value| as_integer(value))
                .map(Urgency::from_level)
                .unwrap_or_default(),
            category: string(&["category"]),
            desktop_entry: string(&["desktop-entry"])
                .map(|entry| entry.trim_end_matches(".desktop").to_owned()),
            image: get(&["image-data", "image_data"]).and_then(|value| image(value)),
            image_path: string(&["image-path", "image_path"]),
            icon_data: get(&["icon_data"]).and_then(|value| image(value)),
            resident: flag("resident"),
            transient: flag("transient"),
            value: get(&["value"])
                .and_then(|value| as_integer(value))
                .map(|value| value.clamp(0, 100) as i32),
            stack_tag: string(&["x-canonical-private-synchronous", "x-dunst-stack-tag"]),
        }
    }
}

/// A variant's string, if it holds one. Empty strings are no string at all.
fn as_string(value: &Value) -> Option<String> {
    match value {
        Value::Str(text) if !text.is_empty() => Some(text.to_string()),
        Value::Value(inner) => as_string(inner),
        _ => None,
    }
}

/// A variant's integer, whichever width the sender happened to use.
fn as_integer(value: &Value) -> Option<i64> {
    Some(match value {
        Value::U8(n) => (*n).into(),
        Value::I16(n) => (*n).into(),
        Value::U16(n) => (*n).into(),
        Value::I32(n) => (*n).into(),
        Value::U32(n) => (*n).into(),
        Value::I64(n) => *n,
        Value::U64(n) => i64::try_from(*n).ok()?,
        Value::Value(inner) => return as_integer(inner),
        _ => return None,
    })
}

/// A variant's boolean. Integers count, as they do to everyone who sends them.
fn as_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::Value(inner) => as_bool(inner),
        other => as_integer(other).map(|n| n != 0),
    }
}

/// `(iiibiiay)`: width, height, rowstride, has alpha, bits per sample, channels, pixels.
fn image(value: &Value) -> Option<Pixmap> {
    let Value::Structure(structure) = value else {
        return match value {
            Value::Value(inner) => image(inner),
            _ => None,
        };
    };
    let fields = structure.fields();
    if fields.len() != 7 {
        return None;
    }
    let int = |index: usize| match &fields[index] {
        Value::I32(n) => Some(*n),
        _ => None,
    };
    let has_alpha = match &fields[3] {
        Value::Bool(flag) => *flag,
        _ => return None,
    };
    let bytes: Vec<u8> = match &fields[6] {
        Value::Array(array) => array
            .iter()
            .map(|byte| match byte {
                Value::U8(b) => Some(*b),
                _ => None,
            })
            .collect::<Option<_>>()?,
        _ => return None,
    };
    Pixmap::from_raw(
        crate::pixmap::Raw {
            width: int(0)?,
            height: int(1)?,
            rowstride: int(2)?,
            has_alpha,
            bits_per_sample: int(4)?,
            channels: int(5)?,
        },
        &bytes,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::StructureBuilder;

    fn hints(pairs: Vec<(&str, Value<'static>)>) -> HashMap<String, OwnedValue> {
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.try_into().unwrap()))
            .collect()
    }

    #[test]
    fn no_hints_is_a_normal_notification() {
        let read = Hints::read(&HashMap::new());
        assert_eq!(read, Hints::default());
        assert_eq!(read.urgency, Urgency::Normal);
    }

    #[test]
    fn urgency_takes_whatever_integer_was_sent() {
        for (value, expected) in [
            (Value::U8(0), Urgency::Low),
            (Value::U8(2), Urgency::Critical),
            // `notify-send -h int:urgency:2` sends an int32, not the byte the spec says.
            (Value::I32(2), Urgency::Critical),
            (Value::I32(1), Urgency::Normal),
            (Value::U32(7), Urgency::Critical),
        ] {
            assert_eq!(
                Hints::read(&hints(vec![("urgency", value)])).urgency,
                expected
            );
        }
        // Not a number at all: ignored, not fatal.
        assert_eq!(
            Hints::read(&hints(vec![("urgency", Value::from("high"))])).urgency,
            Urgency::Normal
        );
    }

    #[test]
    fn both_spellings_of_the_old_hints_are_read() {
        let read = Hints::read(&hints(vec![("image_path", Value::from("/tmp/a.png"))]));
        assert_eq!(read.image_path.as_deref(), Some("/tmp/a.png"));
        let read = Hints::read(&hints(vec![(
            "image-path",
            Value::from("dialog-information"),
        )]));
        assert_eq!(read.image_path.as_deref(), Some("dialog-information"));
    }

    #[test]
    fn a_stack_tag_comes_from_either_convention() {
        let read = Hints::read(&hints(vec![(
            "x-canonical-private-synchronous",
            Value::from("volume"),
        )]));
        assert_eq!(read.stack_tag.as_deref(), Some("volume"));
        let read = Hints::read(&hints(vec![(
            "x-dunst-stack-tag",
            Value::from("brightness"),
        )]));
        assert_eq!(read.stack_tag.as_deref(), Some("brightness"));
    }

    #[test]
    fn flags_and_values() {
        let read = Hints::read(&hints(vec![
            ("resident", Value::Bool(true)),
            ("transient", Value::I32(1)),
            ("value", Value::I32(140)),
            ("desktop-entry", Value::from("org.mozilla.firefox.desktop")),
        ]));
        assert!(read.resident);
        assert!(read.transient);
        assert_eq!(read.value, Some(100), "clamped to a percentage");
        assert_eq!(read.desktop_entry.as_deref(), Some("org.mozilla.firefox"));
    }

    #[test]
    fn image_data_is_decoded() {
        // One opaque red pixel and one half-transparent green one, RGBA.
        let structure = StructureBuilder::new()
            .add_field(2i32)
            .add_field(1i32)
            .add_field(8i32)
            .add_field(true)
            .add_field(8i32)
            .add_field(4i32)
            .add_field(vec![255u8, 0, 0, 255, 0, 255, 0, 128])
            .build()
            .unwrap();
        let read = Hints::read(&hints(vec![("image-data", Value::Structure(structure))]));
        let image = read.image.expect("a valid image-data hint");
        assert_eq!((image.width, image.height), (2, 1));
        assert_eq!(image.get(0, 0), 0xffff0000);
        // Premultiplied: half of 255 green.
        assert_eq!(image.get(1, 0), 0x80008000);
    }

    #[test]
    fn a_malformed_image_is_skipped() {
        let structure = StructureBuilder::new()
            .add_field(10i32)
            .add_field(10i32)
            .add_field(40i32)
            .add_field(true)
            .add_field(8i32)
            .add_field(4i32)
            .add_field(vec![0u8; 12])
            .build()
            .unwrap();
        let read = Hints::read(&hints(vec![("image-data", Value::Structure(structure))]));
        assert_eq!(read.image, None, "too few bytes for 10x10");
    }

    #[test]
    fn actions_pair_up_and_a_dangling_key_is_dropped() {
        let flat: Vec<String> = ["default", "", "reply", "Reply", "orphan"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let paired = actions(&flat);
        assert_eq!(
            paired,
            vec![
                Action {
                    key: "default".into(),
                    label: String::new()
                },
                Action {
                    key: "reply".into(),
                    label: "Reply".into()
                },
            ]
        );
    }
}
