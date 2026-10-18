// SPDX-License-Identifier: GPL-3.0-or-later
//! The hardware keys, as the compositor hands them over.
//!
//! The compositor binds the volume and brightness keys and sends each press here as one line on
//! a datagram socket, `$XDG_RUNTIME_DIR/wlrix-osd.sock`:
//!
//! ```text
//! volume up | volume down | volume mute | mic-mute | brightness up | brightness down
//! ```
//!
//! A datagram rather than a D-Bus call for the reason `wlrix-sound` takes its requests the same
//! way: the sender is the compositor's event loop, which must never wait on anybody, and has no
//! bus connection to make a call on. A send to a local datagram socket returns at once, and fails
//! at once when nobody is listening. The compositor's half is `wlrix-compositor/src/osd.rs`.
//!
//! The socket lives in the per-user runtime directory, which only this user can enter, so a line
//! arriving on it was sent by this user.

use std::os::unix::net::UnixDatagram;
use std::path::PathBuf;

/// The socket, relative to `$XDG_RUNTIME_DIR`. Hand-kept in step with the compositor's copy.
pub const SOCKET_NAME: &str = "wlrix-osd.sock";

/// One key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    VolumeUp,
    VolumeDown,
    VolumeMute,
    MicMute,
    BrightnessUp,
    BrightnessDown,
}

impl Key {
    /// Read one request. Surrounding space is forgiven; anything else unrecognized is `None`.
    pub fn parse(line: &str) -> Option<Self> {
        Some(match line.trim() {
            "volume up" => Key::VolumeUp,
            "volume down" => Key::VolumeDown,
            "volume mute" => Key::VolumeMute,
            "mic-mute" => Key::MicMute,
            "brightness up" => Key::BrightnessUp,
            "brightness down" => Key::BrightnessDown,
            _ => return None,
        })
    }
}

/// Where the socket goes, or `None` without a runtime directory.
pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(SOCKET_NAME))
}

/// Bind the socket, replacing a stale one.
///
/// A socket file left by a daemon that crashed would make the bind fail forever. Only one of
/// these daemons runs at a time -- the notification bus name sees to that, and it is taken before
/// this -- so whatever is at the path now is stale by definition.
pub fn bind() -> std::io::Result<(UnixDatagram, PathBuf)> {
    let path = socket_path().ok_or_else(|| std::io::Error::other("no XDG_RUNTIME_DIR"))?;
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let socket = UnixDatagram::bind(&path)?;
    socket.set_nonblocking(true)?;
    Ok((socket, path))
}

/// Every request waiting on the socket.
pub fn drain(socket: &UnixDatagram) -> Vec<Key> {
    let mut keys = Vec::new();
    let mut buffer = [0u8; 64];
    while let Ok(length) = socket.recv(&mut buffer) {
        match std::str::from_utf8(&buffer[..length])
            .ok()
            .and_then(Key::parse)
        {
            Some(key) => keys.push(key),
            None => eprintln!(
                "wlrix-osd: ignored a request on {SOCKET_NAME}: {:?}",
                String::from_utf8_lossy(&buffer[..length])
            ),
        }
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_the_compositor_sends_is_read() {
        assert_eq!(Key::parse("volume up"), Some(Key::VolumeUp));
        assert_eq!(Key::parse("volume down\n"), Some(Key::VolumeDown));
        assert_eq!(Key::parse("volume mute"), Some(Key::VolumeMute));
        assert_eq!(Key::parse("mic-mute"), Some(Key::MicMute));
        assert_eq!(Key::parse("brightness up"), Some(Key::BrightnessUp));
        assert_eq!(Key::parse("brightness down"), Some(Key::BrightnessDown));
        assert_eq!(Key::parse("volume sideways"), None);
        assert_eq!(Key::parse(""), None);
    }

    #[test]
    fn a_datagram_arrives_as_a_key() {
        let dir = std::env::temp_dir().join(format!("wlrix-osd-control-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(SOCKET_NAME);
        let _ = std::fs::remove_file(&path);
        let socket = UnixDatagram::bind(&path).unwrap();
        socket.set_nonblocking(true).unwrap();
        let sender = UnixDatagram::unbound().unwrap();
        sender.send_to(b"volume up", &path).unwrap();
        sender.send_to(b"nonsense", &path).unwrap();
        sender.send_to(b"brightness down", &path).unwrap();
        assert_eq!(drain(&socket), vec![Key::VolumeUp, Key::BrightnessDown]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
