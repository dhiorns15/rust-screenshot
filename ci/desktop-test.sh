#!/bin/sh
# Runs the desktop tests (src/gallery/desktop_tests.rs): recording, screenshots, the background icon
# and copying files against real desktop services, which the tests start: PipeWire with WirePlumber,
# xdg-desktop-portal, an X server (Xvfb), and a test picture as the screen. This gives them a session bus of their own, and a home folder.
#
# Needs dbus, pipewire, pipewire-bin, pipewire-pulse, pulseaudio-utils, wireplumber, xdg-desktop-portal,
# xvfb, xclip and GStreamer with its PipeWire and PulseAudio plugins (CI's Linux job installs them; see
# ci/Dockerfile).
set -eu
cd "$(dirname "$0")/.."
home=$(mktemp -d)
trap 'rm -rf "$home"' EXIT
# Screenshots, settings and lock files go here, not in the real home folder. A test's name as the
# argument runs just that one.
HOME=$home XDG_CONFIG_HOME=$home/.config TMPDIR=$home USER=rsnip-test \
    dbus-run-session -- cargo test --bin rsnip "${1:-desktop_}" -- --ignored --test-threads=1
