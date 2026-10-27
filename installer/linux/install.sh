#!/bin/sh
# Installs RSnip for the current user (no sudo): the program in ~/.local/bin, and an "RSnip Library"
# entry with its icon in the app menu. Run it from the extracted download:
#
#   ./install.sh               install, or upgrade over an earlier install
#   ./install.sh --check       only check for the libraries and programs RSnip uses
#   ./install.sh --uninstall   remove it again (screenshots and settings are kept)
#
# It never installs system packages itself; if some are missing it says which, and the command
# that installs them on this distribution.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
bin="$HOME/.local/bin"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
desktop="$data/applications/rsnip.desktop"
icon="$data/icons/hicolor/256x256/apps/rsnip.png"
autostart="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/rsnip.desktop"

# Asks RSnip's background icon to quit, if it's running (it checks for this file; a stale one is
# cleared when it next starts). Not `rsnip --tray-quit`, which older versions don't know.
quit_background() {
    echo quit >"${TMPDIR:-/tmp}/rsnip-tray-${USER:-}.request" 2>/dev/null || true
}

# The package manager, from /etc/os-release: apt, dnf, pacman, zypper, or "" if unknown.
manager() {
    ids=$( (. /etc/os-release 2>/dev/null && echo "${ID:-} ${ID_LIKE:-}") || true)
    for id in $ids; do
        case "$id" in
        debian | ubuntu) echo apt; return ;;
        fedora | rhel | centos) echo dnf; return ;;
        arch) echo pacman; return ;;
        suse | opensuse*) echo zypper; return ;;
        esac
    done
}

# The package that provides a library or program, for package manager $1 (generic name if unknown).
package() {
    case "$1:$2" in
    apt:libEGL*) echo libegl1 ;;
    apt:libxkbcommon.so*) echo libxkbcommon0 ;;
    apt:libxkbcommon-x11*) echo libxkbcommon-x11-0 ;;
    apt:libwayland-client*) echo libwayland-client0 ;;
    apt:libwayland-egl*) echo libwayland-egl1 ;;
    apt:libX11.so*) echo libx11-6 ;;
    apt:libX11-xcb*) echo libx11-xcb1 ;;
    apt:libXcursor*) echo libxcursor1 ;;
    apt:libXi.so*) echo libxi6 ;;
    apt:libXrender*) echo libxrender1 ;;
    zypper:libEGL*) echo libEGL1 ;;
    zypper:libxkbcommon.so*) echo libxkbcommon0 ;;
    zypper:libxkbcommon-x11*) echo libxkbcommon-x11-0 ;;
    zypper:libwayland-client*) echo libwayland-client0 ;;
    zypper:libwayland-egl*) echo libwayland-egl1 ;;
    zypper:libX11.so*) echo libX11-6 ;;
    zypper:libX11-xcb*) echo libX11-xcb1 ;;
    zypper:libXcursor*) echo libXcursor1 ;;
    zypper:libXi.so*) echo libXi6 ;;
    zypper:libXrender*) echo libXrender1 ;;
    pacman:libEGL*) echo libglvnd ;;
    pacman:libwayland*) echo wayland ;;
    pacman:libX11*) echo libx11 ;;
    dnf:libEGL* | *:libEGL*) echo libglvnd-egl ;;
    *:libX11-xcb*) echo libX11-xcb ;;
    pacman:lib*) echo "$2" | sed 's/\.so.*//' | tr '[:upper:]' '[:lower:]' ;;
    # Fedora's names, e.g. libXcursor, libxkbcommon-x11.
    *:lib*) echo "$2" | sed 's/\.so.*//' ;;
    *:xdg-open) echo xdg-utils ;;
    *:xdg-user-dir) echo xdg-user-dirs ;;
    *:fc-match) echo fontconfig ;;
    apt:tesseract | zypper:tesseract) echo tesseract-ocr ;;
    pacman:tesseract) echo tesseract tesseract-data-eng ;;
    apt:gst-launch-1.0) echo gstreamer1.0-tools ;;
    dnf:gst-launch-1.0) echo gstreamer1 ;;
    pacman:gst-launch-1.0) echo gstreamer ;;
    zypper:gst-launch-1.0) echo gstreamer-utils ;;
    apt:pipewiresrc) echo gstreamer1.0-pipewire ;;
    dnf:pipewiresrc) echo pipewire-gstreamer ;;
    pacman:pipewiresrc) echo gst-plugin-pipewire ;;
    zypper:pipewiresrc) echo gstreamer-plugin-pipewire ;;
    apt:x264enc) echo gstreamer1.0-plugins-ugly ;;
    dnf:x264enc) echo gstreamer1-plugins-ugly ;;
    pacman:x264enc) echo gst-plugins-ugly ;;
    zypper:x264enc) echo gstreamer-plugins-ugly ;;
    apt:h264parse) echo gstreamer1.0-plugins-bad ;;
    dnf:h264parse) echo gstreamer1-plugins-bad-free ;;
    pacman:h264parse) echo gst-plugins-bad ;;
    zypper:h264parse) echo gstreamer-plugins-bad ;;
    apt:avdec_h264) echo gstreamer1.0-libav ;;
    dnf:avdec_h264) echo gstreamer1-plugin-libav ;;
    pacman:avdec_h264) echo gst-libav ;;
    zypper:avdec_h264) echo gstreamer-plugins-libav ;;
    apt:pulsesrc) echo gstreamer1.0-pulseaudio ;;
    dnf:pulsesrc) echo gstreamer1-plugins-good ;;
    pacman:pulsesrc) echo gst-plugins-good ;;
    zypper:pulsesrc) echo gstreamer-plugins-good ;;
    apt:pactl | dnf:pactl | zypper:pactl) echo pulseaudio-utils ;;
    pacman:pactl) echo libpulse ;;
    # An AAC encoder, for the sound in MP4s: FFmpeg's, which comes with the H.264 decoder.
    apt:avenc_aac) echo gstreamer1.0-libav ;;
    dnf:avenc_aac) echo gstreamer1-plugin-libav ;;
    pacman:avenc_aac) echo gst-libav ;;
    zypper:avenc_aac) echo gstreamer-plugins-libav ;;
    *) echo "$2" ;;
    esac
}

# The arguments without repeats, in order, separated by spaces.
unique() {
    printf '%s\n' "$@" | awk 'NF && !seen[$0]++ { printf "%s%s", sep, $0; sep = " " }'
}

has_library() {
    if command -v ldconfig >/dev/null 2>&1 || [ -x /sbin/ldconfig ]; then
        PATH="$PATH:/sbin:/usr/sbin" ldconfig -p 2>/dev/null | grep -q "/$1\$" && return 0
    fi
    for dir in /usr/lib64 /usr/lib /lib64 /lib /usr/lib/x86_64-linux-gnu /lib/x86_64-linux-gnu; do
        [ -e "$dir/$1" ] && return 0
    done
    return 1
}

# Checks what RSnip loads and runs, and recommends packages for anything missing. Returns 1 if
# RSnip can't start without something.
check() {
    pm=$(manager)
    required="" recommended=""
    # Drawing, and the keyboard, on any desktop.
    libraries="libEGL.so.1 libxkbcommon.so.0"
    if [ -n "${WAYLAND_DISPLAY:-}" ]; then
        libraries="$libraries libwayland-client.so.0 libwayland-egl.so.1"
    else
        libraries="$libraries libX11.so.6 libX11-xcb.so.1 libXcursor.so.1 libXi.so.6 libXrender.so.1 libxkbcommon-x11.so.0"
    fi
    for lib in $libraries; do
        has_library "$lib" || required="$required $(package "$pm" "$lib")"
    done
    # Opening folders and files, finding the Pictures folder, the font for text annotations, and
    # reading the text in screenshots (to copy it, and to search by it).
    for program in xdg-open xdg-user-dir fc-match tesseract; do
        command -v "$program" >/dev/null 2>&1 || recommended="$recommended $(package "$pm" "$program")"
    done
    # Recording the screen: GStreamer, its PipeWire source, and an H.264 encoder with its parser,
    # which is in another package (else it's WebM).
    if command -v gst-inspect-1.0 >/dev/null 2>&1; then
        gst-inspect-1.0 --exists pipewiresrc || recommended="$recommended $(package "$pm" pipewiresrc)"
        gst-inspect-1.0 --exists x264enc || gst-inspect-1.0 --exists openh264enc || recommended="$recommended $(package "$pm" x264enc)"
        gst-inspect-1.0 --exists h264parse || recommended="$recommended $(package "$pm" h264parse)"
        # Playing recordings in the library.
        gst-inspect-1.0 --exists avdec_h264 || gst-inspect-1.0 --exists openh264dec || recommended="$recommended $(package "$pm" avdec_h264)"
        # Sound in recordings: GStreamer's PulseAudio source (PipeWire has a PulseAudio server too),
        # pactl to find it, and an AAC encoder.
        gst-inspect-1.0 --exists pulsesrc || recommended="$recommended $(package "$pm" pulsesrc)"
        command -v pactl >/dev/null 2>&1 || recommended="$recommended $(package "$pm" pactl)"
        gst-inspect-1.0 --exists fdkaacenc || gst-inspect-1.0 --exists avenc_aac || gst-inspect-1.0 --exists voaacenc || recommended="$recommended $(package "$pm" avenc_aac)"
    else
        recommended="$recommended $(package "$pm" gst-launch-1.0) $(package "$pm" pipewiresrc) $(package "$pm" x264enc) $(package "$pm" h264parse) $(package "$pm" avdec_h264) $(package "$pm" pulsesrc) $(package "$pm" pactl)"
    fi
    # Checking for updates.
    if ! command -v curl >/dev/null 2>&1 && ! command -v wget >/dev/null 2>&1; then
        recommended="$recommended curl"
    fi

    case "$pm" in
    apt) install="sudo apt install" ;;
    dnf) install="sudo dnf install" ;;
    pacman) install="sudo pacman -S" ;;
    zypper) install="sudo zypper install" ;;
    *) install="Install these with your package manager:" ;;
    esac
    # One of each, in order.
    required=$(unique $required)
    recommended=$(unique $recommended)
    if [ -n "$required" ]; then
        echo "RSnip needs some libraries that aren't installed, and won't start without them:"
        echo "    $install $required"
    fi
    if [ -n "$recommended" ]; then
        echo "Recommended, for opening folders, the Pictures folder, fonts, copying text, recording (with sound) and update checks:"
        echo "    $install $recommended"
    fi
    if [ -z "$required$recommended" ]; then
        echo "Everything RSnip uses is installed."
    fi
    [ -z "$required" ]
}

refresh() {
    update-desktop-database "$data/applications" 2>/dev/null || true
    gtk-update-icon-cache -q -t "$data/icons/hicolor" 2>/dev/null || true
}

case "${1:-}" in
--uninstall)
    # Stop the icon running in the background, and its start at sign-in.
    quit_background
    rm -f "$bin/rsnip" "$desktop" "$icon" "$autostart"
    refresh
    echo "RSnip is removed. Your screenshots and settings (~/.config/rsnip) are kept."
    exit 0
    ;;
--check)
    check
    exit
    ;;
"") ;;
*)
    sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
    exit 2
    ;;
esac

if [ ! -f "$here/rsnip" ]; then
    echo "Run this from the folder you extracted RSnip into (rsnip should be next to it)." >&2
    exit 1
fi
if [ "$(uname -m)" != x86_64 ]; then
    echo "This download is for x86-64 PCs; on $(uname -m), build RSnip from source (see https://github.com/dhiorns15/rust-screenshot/blob/main/docs/building.md)." >&2
    exit 1
fi
# Running it once catches a C library that's too old for this build, before anything is copied.
probe=$(mktemp -d)
if ! problem=$("$here/rsnip" --write-icon "$probe/icon.png" 2>&1); then
    rm -rf "$probe"
    echo "RSnip can't run on this system:" >&2
    echo "    $problem" >&2
    case "$problem" in
    *GLIBC*)
        echo "This build needs a newer C library (glibc $(getconf GNU_LIBC_VERSION 2>/dev/null | cut -d' ' -f2) is installed)." >&2
        echo "Update your distribution, or build RSnip from source (see https://github.com/dhiorns15/rust-screenshot/blob/main/docs/building.md)." >&2
        ;;
    esac
    exit 1
fi
rm -rf "$probe"

mkdir -p "$bin" "$(dirname "$desktop")" "$(dirname "$icon")"
# Copy then rename, so upgrading works while RSnip is running.
# An earlier version running in the background stops; opening the library starts this one.
quit_background
cp "$here/rsnip" "$bin/rsnip.new"
chmod 755 "$bin/rsnip.new"
mv -f "$bin/rsnip.new" "$bin/rsnip"
"$bin/rsnip" --write-icon "$icon"

cat >"$desktop" <<EOF
[Desktop Entry]
Type=Application
Name=RSnip Library
GenericName=Screenshot library
Comment=Browse, search and annotate your screenshots
Exec="$bin/rsnip" %f
Icon=rsnip
Terminal=false
Categories=Graphics;
MimeType=image/png;image/jpeg;
StartupWMClass=rsnip
Actions=record;record-screen;settings;

[Desktop Action record]
Name=Record an area
Exec="$bin/rsnip" --record

[Desktop Action record-screen]
Name=Record a screen or window
Exec="$bin/rsnip" --record-screen

[Desktop Action settings]
Name=Settings
Exec="$bin/rsnip" --settings
EOF
refresh

echo "RSnip is installed: open \"RSnip Library\" from your app menu, or run rsnip."
if [ -e /usr/bin/rsnip ]; then
    echo "Note: RSnip is also installed from a .deb or .rpm package. Whichever comes first on your PATH runs;"
    echo "      remove one of them (./install.sh --uninstall, or your package manager)."
fi
case ":$PATH:" in
*":$bin:"*) ;;
*) echo "Note: $bin isn't on your PATH yet; sign out and back in, or run $bin/rsnip." ;;
esac
check || true
