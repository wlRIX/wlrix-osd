// SPDX-License-Identifier: GPL-3.0-or-later
//! Turning what a sender named into a picture.
//!
//! `app_icon` and `image-path` are each one of three things, and the specification says only
//! that they may be any of them: an absolute path, a `file://` URI, or an icon name to look up
//! in the theme. Which one is decided by its shape, in that order, so a name that happens to
//! contain a slash is read as the path it looks like rather than searched for.
//!
//! Names go through `freedesktop-icons` with the configured theme first and the bare lookup
//! behind it -- the tray's arrangement, for the tray's reason: almost nothing a sender names is
//! in hicolor, and a bare lookup searches little else. Files and found names are decoded by
//! [`wlrix_ui::image::Images`], which caches by path and size.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use wlrix_ui::image::Images;

use crate::pixmap::Pixmap;

/// The name-to-file lookup and the decoded-image cache behind it.
#[derive(Default)]
pub struct Icons {
    images: Images,
    /// The icon theme to search first.
    theme: String,
    /// Names already resolved, including the ones that resolved to nothing.
    resolved: HashMap<(String, i32), Option<PathBuf>>,
    /// Names already complained about, so a missing icon costs one line rather than one a
    /// notification -- a chat client sends the same missing name with every message.
    reported: HashSet<String>,
}

impl Icons {
    /// The icon theme to search before falling back to hicolor and `/usr/share/pixmaps`.
    pub fn set_theme(&mut self, theme: &str) {
        if self.theme != theme {
            self.theme = theme.to_owned();
            self.clear();
        }
    }

    /// The picture `reference` names, at most `size` square.
    pub fn picture(&mut self, reference: &str, size: i32) -> Option<Pixmap> {
        let path = match file(reference) {
            Some(path) => path,
            None if looks_like_a_name(reference) => self.lookup(reference, size)?,
            None => return None,
        };
        self.images
            .load(&path, size)
            .map(Pixmap::from_image)
            .filter(|pixmap| !pixmap.is_empty())
    }

    /// Find the file an icon name refers to.
    fn lookup(&mut self, name: &str, size: i32) -> Option<PathBuf> {
        let key = (name.to_owned(), size);
        if let Some(found) = self.resolved.get(&key) {
            return found.clone();
        }
        let wanted = size.clamp(1, u16::MAX as i32) as u16;
        let found = (!self.theme.is_empty())
            .then(|| {
                freedesktop_icons::lookup(name)
                    .with_size(wanted)
                    .with_theme(&self.theme)
                    .with_cache()
                    .find()
            })
            .flatten()
            .or_else(|| {
                freedesktop_icons::lookup(name)
                    .with_size(wanted)
                    .with_cache()
                    .find()
            });
        if found.is_none() && self.reported.insert(name.to_owned()) {
            eprintln!("wlrix-osd: no icon found for {name:?}");
        }
        self.resolved.insert(key, found.clone());
        found
    }

    /// Forget everything, so a changed theme or an icon installed since is found next time.
    pub fn clear(&mut self) {
        self.images.clear();
        self.resolved.clear();
        self.reported.clear();
    }
}

/// The file a reference names directly, if it is a path or a `file://` URI and the file is
/// there.
pub fn file(reference: &str) -> Option<PathBuf> {
    let path = if let Some(rest) = reference.strip_prefix("file://") {
        // `file:///home/...` -- the authority is empty for a local file. `file://localhost/...`
        // is the other spelling a URI library may produce.
        let rest = rest.strip_prefix("localhost").unwrap_or(rest);
        PathBuf::from(percent_decode(rest)?)
    } else if reference.starts_with('/') {
        PathBuf::from(reference)
    } else {
        return None;
    };
    (path.is_absolute() && path.is_file()).then_some(path)
}

/// Whether a reference could be an icon name: something without a scheme or a separator. A
/// relative path such as `icons/app.png` would resolve against wherever the daemon happened to
/// start, which is not anywhere the sender meant.
fn looks_like_a_name(reference: &str) -> bool {
    !reference.is_empty() && !reference.contains('/') && !reference.contains(':')
}

/// Undo a URI's `%XX` escapes. `None` when the result is not UTF-8 or an escape is malformed.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// What a `.desktop` file says about the application that sent a notification.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DesktopEntry {
    pub name: Option<String>,
    pub icon: Option<String>,
}

/// Looked-up `.desktop` files, by id, including the ones that were not found.
#[derive(Default)]
pub struct DesktopEntries {
    found: HashMap<String, Option<DesktopEntry>>,
}

impl DesktopEntries {
    /// The entry for `id` (`org.mozilla.firefox`, without `.desktop`), searched for once.
    pub fn get(&mut self, id: &str) -> Option<&DesktopEntry> {
        if !self.found.contains_key(id) {
            let entry = find_desktop_file(id).and_then(|path| read_desktop_file(&path));
            self.found.insert(id.to_owned(), entry);
        }
        self.found.get(id)?.as_ref()
    }

    /// Forget everything, so an application installed since is found.
    pub fn clear(&mut self) {
        self.found.clear();
    }
}

/// Where `.desktop` files live, most specific first: `$XDG_DATA_HOME`, then each of
/// `$XDG_DATA_DIRS` -- which is where Flatpak's exports are too.
fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = std::env::var_os("XDG_DATA_HOME").filter(|dir| !dir.is_empty()) {
        dirs.push(PathBuf::from(home));
    } else if let Some(home) = crate::xdg::home() {
        dirs.push(home.join(".local/share"));
    }
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    dirs.extend(
        system
            .split(':')
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from),
    );
    dirs.into_iter()
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("applications"))
        .collect()
}

/// The first `<id>.desktop` in the search path.
fn find_desktop_file(id: &str) -> Option<PathBuf> {
    // The id comes off the bus from a process this does not control.
    if id.is_empty() || id.contains('/') || id.starts_with('.') {
        return None;
    }
    let file = format!("{id}.desktop");
    application_dirs()
        .into_iter()
        .map(|dir| dir.join(&file))
        .find(|path| path.is_file())
}

/// `Name=` and `Icon=` from the `[Desktop Entry]` group. Localized keys are not read: a
/// notification's title is the application's name, which `Name=` already is.
fn read_desktop_file(path: &Path) -> Option<DesktopEntry> {
    parse_desktop_file(&std::fs::read_to_string(path).ok()?)
}

fn parse_desktop_file(text: &str) -> Option<DesktopEntry> {
    let mut entry = DesktopEntry::default();
    let mut in_group = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_group = line == "[Desktop Entry]";
            continue;
        }
        if !in_group {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Name" if !value.is_empty() => entry.name = Some(value.to_owned()),
            "Icon" if !value.is_empty() => entry.icon = Some(value.to_owned()),
            _ => {}
        }
    }
    (entry != DesktopEntry::default()).then_some(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reference_is_a_path_a_uri_or_a_name() {
        let here = std::env::temp_dir().join(format!("wlrix-osd-icon-{}.png", std::process::id()));
        std::fs::write(&here, b"x").unwrap();
        let text = here.to_string_lossy().to_string();

        assert_eq!(file(&text), Some(here.clone()));
        assert_eq!(file(&format!("file://{text}")), Some(here.clone()));
        assert_eq!(file(&format!("file://localhost{text}")), Some(here.clone()));
        assert_eq!(file("/nonexistent/thing.png"), None);
        assert_eq!(file("dialog-information"), None, "a name is not a file");
        let _ = std::fs::remove_file(&here);

        assert!(looks_like_a_name("dialog-information"));
        assert!(!looks_like_a_name("icons/app.png"));
        assert!(!looks_like_a_name("https://example.org/a.png"));
        assert!(!looks_like_a_name(""));
    }

    #[test]
    fn uris_are_percent_decoded() {
        assert_eq!(
            percent_decode("/home/a/My%20Pictures/%E2%9C%93.png").as_deref(),
            Some("/home/a/My Pictures/✓.png")
        );
        assert_eq!(percent_decode("/bad%2"), None);
        assert_eq!(percent_decode("/bad%zz"), None);
    }

    #[test]
    fn a_desktop_file_gives_its_name_and_icon() {
        let entry = parse_desktop_file(
            "[Desktop Entry]\nType=Application\nName=Firefox\nName[de]=Feuerfuchs\n\
             Icon=firefox\n\n[Desktop Action new-window]\nName=New Window\nIcon=other\n",
        )
        .unwrap();
        assert_eq!(entry.name.as_deref(), Some("Firefox"));
        assert_eq!(
            entry.icon.as_deref(),
            Some("firefox"),
            "actions do not override"
        );
        assert_eq!(parse_desktop_file("[Other]\nName=x\n"), None);
    }

    #[test]
    fn a_desktop_id_cannot_leave_the_applications_directory() {
        assert_eq!(find_desktop_file("../../etc/passwd"), None);
        assert_eq!(find_desktop_file(".hidden"), None);
        assert_eq!(find_desktop_file(""), None);
    }
}
