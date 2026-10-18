#!/usr/bin/env just --justfile
name := 'wlrix-osd'

rootdir := ''
prefix := '/usr'

base-dir := absolute_path(clean(rootdir / prefix))
bin-dir := base-dir / 'bin'
dbusdir := base-dir / 'share' / 'dbus-1' / 'services'

bin-src := 'target' / 'release' / name
bin-dst := bin-dir / name

# The activation file carries an absolute path to the binary, so it is a template with @BINDIR@
# substituted at install time -- the binary's *runtime* path, not where DESTDIR puts it.
#
# Named for this program rather than for the bus name it claims, as mako's
# `fr.emersion.mako.service` is: two files both called `org.freedesktop.Notifications.service`
# would overwrite each other on install, which hides the conflict rather than ending it.
dbus-src := 'data' / 'com.wlrix.Osd.service.in'
dbus-dst := dbusdir / 'com.wlrix.Osd.service'
runtime-bindir := clean(prefix / 'bin')

default:
  @just --list

release:
  cargo build --release

lint:
  cargo clippy --all-targets -- -D warnings

test:
  cargo test

# Install the daemon and its bus activation file.
#
# No `.desktop` entry: the Toolchest builds its menu by scanning them, and this is not something
# a person starts. `wlrix-session` starts it; the activation file covers a notification sent
# before the session got that far, or from outside a wlRIX session.
#
# Deliberately does not build: this is normally run as root, and building as root leaves a target
# directory nobody can write to afterwards.
#
#     just release && sudo just install
[doc("Install the daemon (build first; run as root)")]
install:
  #!/usr/bin/env bash
  set -euo pipefail
  if [ ! -x '{{bin-src}}' ]; then
    echo "no release build -- run 'just release' first" >&2
    exit 1
  fi
  install -Dm0755 '{{bin-src}}' '{{bin-dst}}'
  echo "installed {{bin-dst}}"
  install -d '{{dbusdir}}'
  sed 's|@BINDIR@|{{runtime-bindir}}|g' '{{dbus-src}}' > '{{dbus-dst}}'
  chmod 0644 '{{dbus-dst}}'
  echo "installed {{dbus-dst}}"
  # Another daemon's activation file for the same name means dbus-daemon picks one of the two
  # arbitrarily. Said rather than fixed: the other file belongs to another package.
  for other in '{{dbusdir}}'/*.service; do
    [ "$other" = '{{dbus-dst}}' ] && continue
    if grep -qx 'Name=org.freedesktop.Notifications' "$other" 2>/dev/null; then
      echo
      echo "warning: $other also claims org.freedesktop.Notifications."
      echo "Uninstall that daemon (mako, dunst, ...) or D-Bus may start it instead of this one."
    fi
  done
  echo
  echo "wlrix-session starts this by name off PATH. Settings go in ~/.config/wlrix/osd.toml."

[doc("Remove what install put down")]
uninstall:
  #!/usr/bin/env bash
  set -euo pipefail
  rm -f '{{bin-dst}}' '{{dbus-dst}}'
  echo "removed {{bin-dst}} and {{dbus-dst}}"

clean:
  cargo clean
