# wlrix-osd

Notifications for the wlRIX desktop. The `org.freedesktop.Notifications` server -- what `notify-send`, a browser's
"download complete" and a chat client's new message all go to -- with each notification drawn as a small fixed-size
4Dwm window, stacked in a corner of the screen. It replaces mako or dunst.

- **Language:** Rust
- **License:** GPL-3.0-or-later

IRIX had no notification daemon, so there is no original to copy. The nearest native thing is a small window, and
that is what a notification is: the beveled frame, a titlebar with the application's name, and the message in the
body. Hovering one gives it the active (gold) frame, as focus would; a critical one has it all the time.

The name is for what comes next: on-screen display bars for volume, brightness and the rest of the laptop keys, in
the same program. This is step one -- the notifications. See `_docs/wlrix-osd-roadmap.md` in the workspace.

## What it speaks

The specification, version 1.2: `Notify`, `CloseNotification`, `GetCapabilities`, `GetServerInformation`, and the
`NotificationClosed` and `ActionInvoked` signals, with the right close reason for each way a notification goes.

| Capability                         |                                                                                          |
|------------------------------------|------------------------------------------------------------------------------------------|
| `actions`                          | clicking a notification runs its `default` action. Other actions are not shown yet.     |
| `body`, `body-markup`              | `<b>`, `<i>`, `<u>`, `<a>` and the XML entities; `<img>` becomes its `alt` text          |
| `icon-static`                      | `app_icon`, `image-path` and `image-data` are drawn beside the text                      |
| `x-canonical-private-synchronous`  | the stack tag (below)                                                                    |

Only what is actually done is claimed. Links draw as links but are not clickable, so `body-hyperlinks` is not
advertised; nor are `body-images`, `persistence` or `sound`, which are later steps.

**Hints read:** `urgency` (byte or int), `category`, `desktop-entry`, `image-data` / `image_data`, `image-path` /
`image_path`, `icon_data`, `resident`, `transient`, `value` (drawn as a progress bar), and the stack tags
`x-canonical-private-synchronous` and `x-dunst-stack-tag`. A new notification with the same stack tag from the same
application takes the old one's place instead of piling up under it -- how a volume script updates one bubble. The
one it replaced is reported closed with reason 4.

**The picture** is the first of these that resolves, in the specification's order: `image-data`, `image-path`,
`app_icon`, `icon_data` -- then the icon in the sender's `.desktop` file when it named a `desktop-entry`. A path, a
`file://` URI or a themed icon name all work. Pictures are shrunk to fit, keeping their shape, and never enlarged.

## Using it

|                                | |
|--------------------------------|-|
| left click on the window-menu button | dismiss |
| left click anywhere else       | run the `default` action if there is one, then dismiss (unless `resident`) |
| right click                    | dismiss |
| hover                          | holds its timer; the rest of the time comes back when the pointer leaves |

**A timer starts when the notification is on screen, not when it arrives.** One waiting below a full stack keeps
its whole lifetime until it is shown, and so does one that arrived while the monitors were blanked. Critical
notifications stay until dismissed, whatever timeout their sender passed, as the specification asks.

## Where it sits

One **wlr-layer-shell overlay surface**, anchored to a corner, exactly as big as the stack. Overlay so it is seen
over everything; no exclusive zone, so nothing moves out of its way; **no keyboard**, ever, so it cannot take focus
from what someone is typing.

The gaps between notifications are transparent and are cut out of the surface's input region, so a click in one
reaches the window underneath. That relies on `wlrix-compositor` moving on to the top layer and the windows when an
overlay surface's input region rejects a point, which it does.

Every layer surface is closed when the monitors blank. The notifications are kept and the surface is rebuilt when an
output comes back.

## Configuration

`~/.config/wlrix/osd.toml`, then `/etc/wlrix/osd.toml`. The first file found wins outright, unknown keys are
refused, and `wlrix-osd --check-config <path>` says whether a file would be accepted.

```toml
output = "DP-1"              # which monitor; default is the one at the origin
anchor = "top-right"         # top-right | top-left | bottom-right | bottom-left
max_visible = 5              # on screen at once; the rest wait their turn
width = 360                  # one notification, frame included

[timeout]                    # seconds, when the sender leaves it to the server; 0 = until dismissed
low = 5
normal = 8
critical = 0                 # used whatever the sender asked for

[appearance]
palette = "gotham"           # the color scheme; default is "classic"
icon_theme = "wlrix"         # where an icon name is looked up; "" for none

[metrics]
icon = 48                    # the picture beside the text, square
gap = 6                      # between notifications
margin = 12                  # between the stack and the screen edges
body_lines = 6               # the body is cut off, with an ellipsis, past this
```

`SIGHUP` re-reads the file; the pidfile is `$XDG_RUNTIME_DIR/wlrix-osd.pid`.

## Installing

```console
$ just release && sudo just install
```

The binary, and `com.wlrix.Osd.service` so D-Bus can start it for a notification sent before the session did.

**Only one program can be the notification server.** mako and dunst are D-Bus activated, and with two activation
files claiming `org.freedesktop.Notifications` dbus-daemon picks one arbitrarily. Uninstall the other one; `install`
warns if it finds one. If one is already running, `wlrix-osd` refuses to start and says so, and `--replace` takes the
name over.

## Testing it

The parts that need no bus or screen are unit-tested: the markup scanner against malformed bodies, hint reading
for every integer width senders use, `image-data` with padded rows, the stack's ordering, replacement and expiry
clocks with made-up instants, the text wrapping and every piece of the layout.

```console
$ cargo test && cargo clippy --all-targets -- -D warnings && cargo fmt --all --check
```

The rest needs a session. Under a nested compositor and a private bus, so the live session's daemon keeps its name:

```console
$ dbus-run-session -- sh -c 'WAYLAND_DISPLAY=wayland-2 wlrix-osd & sleep 1; notify-send -u critical Hi there'
```

`notify-send` covers most of it: `-u critical`, `-t 1500`, `-h int:value:40`, `-h string:x-dunst-stack-tag:vol`,
`-h string:image-path:/path/to.png`, `-r <id>` to replace. `dbus-monitor "interface='org.freedesktop.Notifications'"`
shows the close reasons.

## Threads

The tray's shape: zbus dispatches method calls on its own thread, which allocates the id `Notify` has to return,
converts the arguments, and sends the result down a `calloop::channel`; the main loop owns every piece of state.
Signals are emitted from the loop, which does not wait on anyone to do it. One more thread waits for `NameLost`: a
daemon started later with `--replace` takes the name without asking, and this one then exits rather than go on
holding notifications nobody can reach.
