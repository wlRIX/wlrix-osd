// SPDX-License-Identifier: GPL-3.0-or-later
//! wlRIX notification daemon.
//!
//! Owns `org.freedesktop.Notifications` and draws each notification as a small 4Dwm window in a
//! corner of the screen. A Wayland client and a D-Bus service at once; see [`wlrix_osd::ui`] and
//! [`wlrix_osd::dbus`].

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut replace = false;
    let mut rest = args.iter();
    while let Some(argument) = rest.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                println!(
                    "wlrix-osd {}\n\n\
                     Notifications for the wlRIX desktop: the org.freedesktop.Notifications\n\
                     server, drawn as 4Dwm windows. Started by wlrix-session; needs a\n\
                     running compositor with wlr-layer-shell and a session D-Bus.\n\n\
                     Usage: wlrix-osd [options]\n\n\
                     Options:\n  \
                       --replace              take org.freedesktop.Notifications from\n                         \
                                              whatever holds it, and carry on\n  \
                       --check-config <path>  say whether that file would be accepted, exit\n  \
                       -h, --help             this message\n  \
                       -V, --version          print the version\n\n\
                     Settings live in ~/.config/wlrix/osd.toml, or /etc/wlrix/osd.toml.",
                    env!("CARGO_PKG_VERSION")
                );
                return;
            }
            "--version" | "-V" => {
                println!("wlrix-osd {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            // Answers a question about a file and starts nothing. `wlrix-settings-daemon` runs
            // this against a candidate file before renaming it into place.
            "--check-config" => {
                let Some(path) = rest.next() else {
                    eprintln!("wlrix-osd: --check-config needs a path");
                    std::process::exit(2);
                };
                if let Err(why) = wlrix_osd::config::check(std::path::Path::new(path)) {
                    eprintln!("{why}");
                    std::process::exit(1);
                }
                return;
            }
            "--replace" => replace = true,
            other => {
                eprintln!("wlrix-osd: unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }

    if let Err(err) = wlrix_osd::ui::run(replace) {
        eprintln!("wlrix-osd: {err}");
        std::process::exit(1);
    }
}
