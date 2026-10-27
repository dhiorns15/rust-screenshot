# RSnip on Linux

## What works on Linux

The library and editor also run on Linux (X11 or Wayland), and take screenshots and record the
screen. RSnip keeps them in its folder, `~/RSnip` unless you choose another in Settings → Library &
editor, and annotates what's there. The desktop's own screenshot tool (or Flameshot,
`grim`/`slurp`, …) still works: it keeps saving to the Pictures folder's `Screenshots`, so RSnip
moves each new screenshot from there into its folder as it's taken (*Bring in the desktop's
screenshots*, in the same place; the background icon or the library does it). Reading the text in screenshots (to copy it, or
to search by it) uses [Tesseract](https://github.com/tesseract-ocr/tesseract) when it's installed
(`tesseract-ocr` on Debian and Ubuntu; the installer's check recommends it). Copying as files
(screenshots, recordings, several from the library) works as on Windows, to paste into a folder,
chat or email; pinning and dragging files out are Windows-only.

To build it yourself instead, see [Building](building.md#on-linux).

## Screenshots on Linux

Run `rsnip --region` and drag over the part of the screen to capture, or
`rsnip --full` for all of it, `rsnip --delay 5` for a region in 5 seconds, `rsnip --text` to copy
the text in an area (it needs Tesseract), or `rsnip --last` for the same area as last time. As on
Windows, the screenshot is copied to the clipboard and saved to RSnip's folder, and a notification
says so. To get them on keys, add custom shortcuts in your desktop's settings: on GNOME,
*Settings → Keyboard → Keyboard Shortcuts → Custom Shortcuts*, with a command like `rsnip --region`
(its full path, if RSnip isn't on your `PATH`). RSnip asks the desktop for the
screenshot (its screenshot portal, which Wayland desktops and most X11 ones have), so GNOME may ask
once whether RSnip may take screenshots. If the desktop won't (Ubuntu 24.04's GNOME can refuse
without asking, or take a while to), RSnip reads the screen itself on an X11 desktop, or on Wayland
asks you to pick the screen to share, as for recording. RSnip's icon takes it, starting if it isn't running, so
that the copy stays on the clipboard.

## Recording on Linux

From RSnip's icon in the top bar or panel, choose *Record an area…* or
*Record screen or window…* (or the same from RSnip's app-menu entry, or run `rsnip --record` /
`rsnip --record-screen`, which you can give a keyboard shortcut in your desktop's settings).
Your desktop asks which screen or window to share, as it does for screen sharing in a browser; for
an area, RSnip then shows that screen to drag over. The icon gets a red dot while recording: click
it (or *Stop recording*, or run the command again) to finish. The recording is saved to RSnip's
folder in the format set in Settings → Recording and copied as a file, ready to paste, and a
notification says so. Recordings use
[GStreamer](https://gstreamer.freedesktop.org/) with its PipeWire plugin, as the desktop's own
recorder does:

| | Debian, Ubuntu | Fedora | Arch | openSUSE |
| --- | --- | --- | --- | --- |
| Recording | `gstreamer1.0-tools gstreamer1.0-pipewire` | `gstreamer1 pipewire-gstreamer` | `gstreamer gst-plugin-pipewire` | `gstreamer-utils gstreamer-plugin-pipewire` |
| H.264 | `gstreamer1.0-plugins-ugly gstreamer1.0-plugins-bad` | `gstreamer1-plugin-openh264 gstreamer1-plugins-bad-free` | `gst-plugins-ugly gst-plugins-bad` | `gstreamer-plugins-ugly` (Packman) `gstreamer-plugins-bad` |
| H.265 | `gstreamer1.0-plugins-bad` | `gstreamer1-plugins-bad-freeworld` (RPM Fusion) | `gst-plugins-bad` | `gstreamer-plugins-bad` (Packman) |
| Sound | `gstreamer1.0-pulseaudio pulseaudio-utils gstreamer1.0-libav` | `gstreamer1-plugins-good pulseaudio-utils gstreamer1-plugin-libav` | `gst-plugins-good libpulse gst-libav` | `gstreamer-plugins-good pulseaudio-utils gstreamer-plugins-libav` |

Without an H.264 or H.265 encoder, recordings are WebM (VP8), which every GStreamer install can
write; the graphics card's encoder (VA-API) is used when it's there and the others aren't. Each
encoder needs its parser too (`h264parse`, in plugins-bad). Settings → Recording says what's
missing, and `rsnip --check-recording` records a moment of a test picture the way a recording
would be made, then says what it came out as.

### Sound

MP4 and WebM recordings include what the computer plays, which is on by default, and the
microphone, which is off. They're mixed into one track: AAC in MP4s, Opus in WebMs. GIFs have no
sound. Turn either on or off, and pick the output and the microphone (the default ones unless you choose: with more than one output, pick the one you hear), in
Settings → Recording, or on the Video tab of *Capture panel…* in the icon's menu.

Sound comes from the desktop's PulseAudio server, which PipeWire provides as `pipewire-pulse` on
current desktops. GStreamer's PulseAudio plugin reads it, `pactl` lists the outputs and microphones, and MP4s
need an AAC encoder: `fdkaacenc`, `avenc_aac` (in `gstreamer1.0-libav`) or `voaacenc` (in
`gstreamer1.0-plugins-bad`), whichever is installed. The Sound row above has them; the `.deb` and
`.rpm` recommend them, and `install.sh` checks for them. Ubuntu doesn't have
`fdkaacenc`, but the other two are there.

Sound doesn't stop a recording from starting: if there's no AAC encoder, the sound server can't be
reached, the microphone isn't connected, or the sound fails as the recording starts, RSnip records
without that part and says so in the "Recording saved" notification. Quiet stretches, a still
screen and a microphone that sends nothing are recorded as silence, so the sound stays in step with
the picture. `rsnip --check-recording` also records a test tone and says whether sound works, and
which microphones it found.

If GStreamer stops part way through a recording (the sound server goes away, say), an MP4 can't be
played, because its index is written when it's finished: RSnip says it couldn't save the recording
rather than keeping a file that won't open. A WebM is kept, and the notification says its end may
be missing.


## The icon in the top bar or panel

Closing the library leaves RSnip's icon in the top bar or panel, for recording and opening the
library, Settings or the screenshots folder again; *Capture panel…* in its menu opens a small window
to take a screenshot or start a recording with the options that matter for it (the format and whether it has sound, say), which its pin keeps open when you click elsewhere (Capture still closes it, so it is not in the picture). Your desktop places it each time it opens; drag it by any empty part to move it. On X11 it stays above other windows and out of the dock and Alt+Tab. Clicking the icon itself opens the library; *Quit RSnip* in its menu stops it. Ubuntu, KDE and most panels show
it; on plain GNOME (e.g. Fedora) it needs the
[AppIndicator extension](https://extensions.gnome.org/extension/615/appindicator-support/). Settings →
General turns it off, or starts it when you sign in.

## Installing on Linux

The downloads run on any x86-64 distribution with glibc 2.39 or later (for example Ubuntu 24.04,
Debian 13, Fedora 40, openSUSE Tumbleweed, Arch, or newer).

**Ubuntu, Debian and others that use `.deb`**: download `rsnip_X.Y.Z-1_amd64.deb` from the
[latest release](https://github.com/dhiorns15/rust-screenshot/releases/latest) and open it (it opens
in your software centre), or run:

```
sudo apt install ./rsnip_X.Y.Z-1_amd64.deb
```

**Fedora, openSUSE and others that use `.rpm`**: download `rsnip-X.Y.Z-1.x86_64.rpm` and open it,
or run one of:

```
sudo dnf install ./rsnip-X.Y.Z-1.x86_64.rpm
sudo zypper install --allow-unsigned-rpm ./rsnip-X.Y.Z-1.x86_64.rpm
```

The package brings what RSnip uses with it: GStreamer for recording (with an H.264 encoder on Ubuntu,
Debian and Fedora; on openSUSE that comes from Packman) and Tesseract for reading text. It adds
**RSnip Library** (with *Record* and *Settings* actions) to your app menu. Install a newer download
the same way to upgrade, and remove it with `sudo apt remove rsnip` / `sudo dnf remove rsnip` (or
from your software centre); your screenshots and settings are kept.

**Elsewhere** (Arch, say), or to install for your user only without `sudo`, use the `.tar.gz`:

```
tar -xzf rsnip-vX.Y.Z-linux-x64.tar.gz
cd rsnip
./install.sh
```

This copies `rsnip` to `~/.local/bin` and adds **RSnip Library** (with its icon, and *Record* and
*Settings* actions) to your app menu, so you can pin it to the dock. It then checks for what RSnip
uses (the graphics and keyboard libraries for your session, X11 or Wayland, plus `xdg-open`,
`fc-match`, `curl`/`wget`, Tesseract and GStreamer) and, if anything is missing, prints the command
that installs it on your distribution (apt, dnf, pacman or zypper). It never installs system
packages itself, and stops before copying anything if this build can't run on your system (e.g. an
older glibc).

- `./install.sh --check` only runs that check.
- Run `./install.sh` from a newer download to upgrade.
- `./install.sh --uninstall` removes it (and stops it running in the background); your screenshots
  and settings are kept.

Then point your desktop's screenshot tool at RSnip's folder (or RSnip at your tool's, in Settings →
Library & editor), and screenshots show up in the library. To open a capture straight in the
editor, bind a key to a command such as `grim -g "$(slurp)" ~/RSnip/shot-$(date +%s).png && rsnip`
(Wayland, wlroots desktops) or `flameshot gui -p ~/RSnip`. `rsnip file.png` opens one file.

## Settings and updates on Linux

Settings live in `~/.config/rsnip/settings.json`, or open *Settings* from the library. Only you can
read it. If it's ever damaged, RSnip keeps it as `settings.json.bak` and starts new settings. RSnip's
windows and its background icon leave each other requests, and hold their lock files, in
`$XDG_RUNTIME_DIR/rsnip` (`/run/user/<your user id>/rsnip`), a folder that is yours alone.

When a new version is out, the library's status bar links to it. It checks once each time the
library opens, using `curl` or, if that isn't installed (as on Ubuntu), `wget`; turn it off in *Settings → Updates*.
If RSnip came from its `.deb` or `.rpm`, the link opens *Settings → Updates*, where *Install* downloads the new package,
checks it against GitHub's SHA-256, installs it with your package manager (asking for your
password) and offers to restart RSnip. If it can't ask for your password (no authentication agent is
running, as can happen over remote desktop), or the package manager fails (another update running,
say), the downloaded package is kept, and Settings shows the command that installs it in a terminal,
such as `sudo apt install /tmp/rsnip-update-…/rsnip-X.Y.Z.deb`. Other copies can't update
themselves: download the new version and run `./install.sh` again.

Upgrading the package stops RSnip's icon in the top bar or panel, which was running the old program.
After *Install* in Settings, Settings starts it again from the new one; after an upgrade from the
terminal or your software centre, it's back when the library next opens (or when you sign in, if
it starts then).

Passwords and keys for *Push to* and *Descriptions* are kept in the desktop's keyring (GNOME
Keyring, KWallet: any Secret Service). Over remote desktop (xrdp) the login keyring is often locked:
Settings asks for it to be unlocked when it needs one of them, and says *The keyring is locked* if
that isn't answered within a minute. Automatic pushes and descriptions never ask; they wait, and
carry on within a minute of it being unlocked, say with *Test* in Settings (see
[Places to push to](destinations.md#the-places)).
