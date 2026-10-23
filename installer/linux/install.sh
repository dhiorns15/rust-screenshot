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
        echo "Recommended, for opening folders, the Pictures folder, fonts, copying text and update checks:"
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
    rm -f "$bin/rsnip" "$desktop" "$icon"
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
    echo "This download is for x86-64 PCs; on $(uname -m), build RSnip from source (see README.md)." >&2
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
        echo "Update your distribution, or build RSnip from source (see README.md)." >&2
        ;;
    esac
    exit 1
fi
rm -rf "$probe"

mkdir -p "$bin" "$(dirname "$desktop")" "$(dirname "$icon")"
# Copy then rename, so upgrading works while RSnip is running.
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
Categories=Graphics;Utility;
MimeType=image/png;image/jpeg;
StartupWMClass=rsnip
Actions=settings;

[Desktop Action settings]
Name=Settings
Exec="$bin/rsnip" --settings
EOF
refresh

echo "RSnip is installed: open \"RSnip Library\" from your app menu, or run rsnip."
case ":$PATH:" in
*":$bin:"*) ;;
*) echo "Note: $bin isn't on your PATH yet; sign out and back in, or run $bin/rsnip." ;;
esac
check || true
