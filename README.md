# RSnip

[![CI](https://github.com/dhiorns15/rust-screenshot/actions/workflows/ci.yml/badge.svg)](https://github.com/dhiorns15/rust-screenshot/actions/workflows/ci.yml)

A small, open-source, Snagit-style screenshot tool for Windows and Linux, written in Rust. On
Windows it does everything below; on Linux it is the library and editor for screenshots taken with
your desktop's own tool (see [Linux](#linux)).

- **Takes over Print Screen** (and optionally **Win+Shift+S**) from the built-in Snipping Tool.
- **Freezes the screen** and lets you drag a region, or click a window/monitor to grab it.
- **Copies every capture to the clipboard** and saves a PNG to your Screenshots folder.
- **Shortcuts for every action**: Print Screen can capture a region, all screens, record and more,
  each on its own key, set in the Settings window.
- **Library and editor**: browse every screenshot and annotate it with arrows, lines, boxes,
  ellipses, highlighter, blur, pixelate, redact, spotlight, pen, text, numbered steps and crop, with undo/redo. Everything stays
  selectable to move, resize, restyle or delete.

## Download

From the [Releases page](https://github.com/dhiorns15/rust-screenshot/releases), on Windows get either:

- **The installer**, `rsnip-vX.Y.Z-windows-x64-setup.exe` (Windows 10 or later, 64-bit). It installs
  for your user only (no admin prompt) into `%LOCALAPPDATA%\Programs\RSnip`, adds *RSnip* and
  *RSnip Library* to the Start menu, and can start RSnip when you sign in. Running a newer installer
  upgrades in place, closing RSnip first. Uninstall it from *Settings → Apps*; your screenshots and
  settings are kept.
- **`rsnip.exe`** on its own: a single self-contained exe; put it anywhere and run it.

On Linux (x86-64), get `rsnip-vX.Y.Z-linux-x64.tar.gz` and see [Installing on Linux](#installing-on-linux).

## Build

On Windows this needs the Rust MSVC toolchain.

```
cargo build --release
```

The binary is `target\release\rsnip.exe`. To package it as an installer (needs
[Inno Setup 6](https://jrsoftware.org/isinfo.php), e.g. `winget install JRSoftware.InnoSetup`):

```
powershell -File installer\build.ps1
```

The installer is written to `target\installer`.

### Linux

The library and editor also run on Linux (X11 or Wayland). Capturing is left to your desktop's
screenshot tool (or Flameshot, `grim`/`slurp`, …): RSnip browses `~/Pictures/Screenshots` and
annotates what's there. Pinning, OCR, copying as files, dragging files out, and capture/recording
are Windows-only.

#### Installing on Linux

The download runs on any x86-64 distribution with glibc 2.39 or later (for example Ubuntu 24.04,
Debian 13, Fedora 40, Arch, openSUSE Tumbleweed, or newer). It installs for your user only, no
`sudo`:

```
tar -xzf rsnip-vX.Y.Z-linux-x64.tar.gz
cd rsnip
./install.sh
```

This copies `rsnip` to `~/.local/bin` and adds **RSnip Library** (with its icon, and a *Settings*
action) to your app menu, so you can pin it to the dock. It then checks for what RSnip uses (the
graphics and keyboard libraries for your session, X11 or Wayland, plus `xdg-open`, `fc-match` and
`curl`/`wget`) and, if anything is missing, prints the command that installs it on your
distribution (apt, dnf, pacman or zypper). It never installs system packages itself, and stops
before copying anything if this build can't run on your system (e.g. an older glibc).

- `./install.sh --check` only runs that check.
- Run `./install.sh` from a newer download to upgrade.
- `./install.sh --uninstall` removes it; your screenshots and settings are kept.

Then use your desktop's screenshot tool as usual; screenshots saved to `~/Pictures/Screenshots` show
up in the library. To open a capture straight in the editor, bind a key to a command such as
`grim -g "$(slurp)" ~/Pictures/Screenshots/shot-$(date +%s).png && rsnip` (Wayland, wlroots
desktops) or `flameshot gui -p ~/Pictures/Screenshots`. `rsnip file.png` opens one file.

Settings live in `~/.config/rsnip/settings.json`, or open *Settings* from the library. When a new
version is out, the library's status bar links to it. It checks once each time the library opens,
using `curl` or, if that isn't installed (as on Ubuntu), `wget`; turn it off in *Settings → Updates*.
There is no self-updater on Linux: download the new version and run `./install.sh` again.

To build it yourself instead:

```
# Needs Rust (rustup.rs) and a C compiler, e.g. build-essential or gcc.
cargo build --release
./target/release/rsnip [file]
```

### Tests

`cargo test` runs everything that works on your system, including tests that drive the whole library
window headlessly through its accessibility tree. Pixel snapshots of the window
(`tests/snapshots`) run on Linux, in the image CI uses:

```
docker build -t rsnip-test ci
docker run --rm -v "$PWD:/work" -w /work -e CARGO_TARGET_DIR=/work/target/linux rsnip-test cargo test
```

After an intended UI change, add `-e UPDATE_SNAPSHOTS=1` and check every changed PNG. To look at the
window without opening it, render it to a PNG:

```
docker run --rm -v "$PWD:/work" -w /work -e CARGO_TARGET_DIR=/work/target/linux   -e "RSNIP_UI=size=1280x820 dock=bottom click=Text" -e RSNIP_UI_OUT=target/ui.png   rsnip-test cargo test render_window -- --ignored
```

To publish a release, bump `version` in `Cargo.toml`, commit, then `git tag vX.Y.Z && git push --tags`.
The release workflow runs the tests and attaches the Windows installer, exe and zip, and a Linux
build, to a new GitHub Release. It then installs the previous release on a fresh machine and checks
that `rsnip --update` updates it to the new one.

## Usage

Run `rsnip.exe`. It sits in the notification area.

| Action | How |
| --- | --- |
| Region capture | Print Screen (or the shortcut you chose), Win+Shift+S, or left-click the tray icon |
| Select | Drag a rectangle, or click to capture the highlighted window. If other windows overlap it, it is brought to the front and captured cleanly |
| One screen | Press Enter or F in the selection overlay to capture the monitor under the mouse |
| All screens | Press A in the selection overlay, or use *Capture all screens* in the tray menu |
| Copy text (OCR) | Tray menu → *Capture text (OCR)*: select an area and its text is copied to the clipboard. In the editor, *Export → Copy text (OCR)* reads the whole image. Uses the OCR built into Windows |
| Pin to screen | Press P in the selection overlay to pin the highlighted area, or use *Export → Pin to screen* in the editor or a thumbnail's right-click menu. The pin floats above other windows: drag to move, mouse wheel to fade, Ctrl+wheel to zoom, right-click to copy or save, double-click or Esc to close |
| Fine-tune | A magnifier next to the cursor shows the pixels under it with their position and colour (M hides or shows it; Settings → Capture sets whether it starts shown). Arrow keys move the mouse 1 px, Shift+arrows 10 px, including while dragging. C copies the colour under the cursor as #RRGGBB |
| Cancel | Esc or right-click |
| Same area again | Tray menu → *Capture last region*, or R in the selection overlay, captures exactly the area you captured last time |
| Scrolling capture | Tray menu → *Scrolling capture*, then click a window (or drag over the part that scrolls). RSnip scrolls it to the end and stitches one tall image; sticky headers and footers appear once. Esc stops early |
| Record video | Tray menu → *Record screen area*, pick an area, and it records to MP4 (or GIF, chosen in Settings), mouse pointer included. A red frame marks the area and a bar shows the time; click *Stop* or press a capture shortcut to finish. The video is saved to the screenshots folder and copied as a file, ready to paste |
| Delayed capture | Tray menu → *Delayed capture* → 3, 5 or 10 seconds, to open a menu or tooltip first. The countdown shows in the tray tooltip |
| Open library | Tray menu → *Open library & editor*, or click the "saved" notification |
| Update | When a new version is out, a notification says so (once per version) and the tray menu offers *Update to RSnip x.y.z…*. If you used the installer, clicking either downloads the new installer, checks its SHA-256 against GitHub's, installs it and restarts RSnip. A standalone `rsnip.exe` opens the download page instead |

Alt+PrtScn and Win+PrtScn are left alone and keep their Windows behaviour.

### Settings

Tray menu → *Settings…* (or the gear in the library, or `rsnip --settings`) opens the Settings window.
Changes apply straight away.

| Section | |
| --- | --- |
| General | **Theme**: System (follows Windows' light or dark mode), Light or Dark. **Start with Windows**, **Open the library when RSnip starts** (otherwise it starts quietly in the tray), **Show notifications** |
| Capture | **Save screenshots to the folder** (when off, captures go to the clipboard only), **Open the editor after capture** (this saves the capture even if saving is off), **Show the magnifier** (or press M while selecting). Every capture is copied to the clipboard |
| Shortcuts | A shortcut for each action (region, all screens, last region, scrolling, record, text). *Change* asks you to press the new combination (Esc cancels); giving it a key another action has moves it. **Also take over Win+Shift+S** for a region capture. It warns if Windows' own Print Screen setting is on or two actions share a key |
| Recording | MP4 or GIF |
| Library & editor | Library on the left or along the bottom; **Read text in screenshots for search**; border and drop shadow on exports |
| Updates | **Check automatically** (on by default: shortly after RSnip starts, then daily, it asks GitHub for the latest release; nothing else is sent), **Install automatically** (off by default; installed copies only: installs a new version as soon as it's found, unless you're in the middle of something), *Check now* |
| System info | What a bug report needs: RSnip's version and install, the OS, CPU, memory, graphics driver, display scale, your settings, the last crash and where files are kept. *Copy* puts it on the clipboard; *Report a problem* copies it and opens a new GitHub issue to paste it into. It has no screenshots or file names, and your home folder shows as `~` |
| About | Version and links |

Giving an action Print Screen is how you choose what Print Screen does, e.g. PrtScn for all screens
and Shift+PrtScn to record. By default PrtScn captures a region and the other actions have no
shortcut. While recording, any capture shortcut stops it.

Settings are stored in `HKCU\Software\RSnip` (on Linux, `~/.config/rsnip/settings.json`).

If RSnip crashes, it writes what happened to a crash report in `%LOCALAPPDATA%\RSnip\crashes` (on
Linux, `~/.local/state/rsnip/crashes`; the last 10 are kept). Next time, the tray app or the library
says so, and *Settings → System info → Report this crash* copies the report for a GitHub issue. A
report says where in RSnip's code it crashed; it has no screenshots or file names. Crashes outside
RSnip's own code, such as in a graphics driver, are reported too.

### Editor

The toolbar has the tools as icons on the left, grouped as selecting, drawing, text and marking up,
hiding, and cropping (hover one for its name, key and how to use it), and undo/redo, *Copy*, *Save* and
the *Export* menu on the right. The row below shows only the options for the current tool or selected
annotation (colour, size, shadow, delete) and a hint on how to use it.

The library sits on the left, or along the bottom as a filmstrip so the editor gets the full width:
switch with the two layout buttons next to *Library*. RSnip remembers the choice.

| Tool | Key | |
| --- | --- | --- |
| Select | V | click an annotation to select it; drag to move it, drag its handles to resize it (arrows: drag either end) |
| Arrow | A | drag; hold Shift to snap to 45° |
| Line | L | drag; hold Shift to snap to 45° |
| Box | B | drag |
| Ellipse | E | drag |
| Pen | P | freehand |
| Text | T | click, type, Enter. *Shadow* (on by default) adds a soft drop shadow for readability. Click existing text (or double-click it with any shape tool) to select and edit it; drag text to move it. While selected, resize it with the Size slider or the corner handle, and move it with the grip on its top-left corner. Clear it to delete |
| Step | N | click to drop 1, 2, 3… markers |
| Highlight | H | drag; multiply-blended like a real highlighter |
| Spotlight | S | drag; dims everything outside the area |
| Blur | U | drag; strong enough to make text unreadable |
| Pixelate | X | drag; mosaic over sensitive content |
| Redact | R | drag; a solid box (black by default) that hides content for certain |
| Crop | C | drag to select an area, adjust it by dragging its edges, corners or middle, then click *Crop* (or press Enter, or double-click inside it). *Cancel* or Esc discards it. Undoable |

Whatever you just drew stays selected (except pen strokes), so you can adjust it straight away; the
Select tool picks any annotation. While something is selected, the options row's colours, *Size* and
*Shadow* change it, Delete removes it, arrow keys nudge it (Shift: 10 px) and Esc deselects it.
Each move, resize or restyle is one undo step.

Annotations stay editable after saving: the saved PNG is flattened for sharing, and the original
image plus your annotations are kept in a hidden `.rsnip` folder beside it, so reopening the
screenshot lets you keep editing. If the PNG is changed by another program, it opens as a plain image. Editing data for screenshots
deleted outside RSnip is cleaned up automatically.

Ctrl+Z / Ctrl+Y undo/redo, Ctrl+C copies the edited image, Ctrl+S overwrites the file,
Ctrl+Shift+S saves a copy, Ctrl+scroll zooms, Ctrl+0 fits to the window.
The *Export* menu saves a copy (PNG or JPG), copies the image *as a file* so it pastes as an
attachment into a folder, chat or email, copies the text in it (OCR), or pins it to the screen. Its
*Border* and *Drop shadow* options are added to every copy, save and export (the annotations stay
editable without them).
Search finds screenshots by the words in them, the app and window they were captured from, and their name
(every word must match something, in any order). RSnip reads the text with Windows' built-in text
recognition, in the background, and keeps it beside the screenshots in the hidden `.rsnip` folder; nothing
leaves your PC. Hover a result to see where it came from and the line that matched. On Linux, search uses
names only.
The library groups screenshots by date (Today, Yesterday, This week, …). Ctrl+click and Shift+click
select several, to copy them as files, delete them, pin them or drag them out together.
Recordings appear with an MP4/GIF badge and open in your video player.
Drag a library thumbnail into Explorer, a chat app or an email to drop the file there.
Right-click a library thumbnail to open it externally, show it in Explorer, pin it, or move it to the Recycle Bin.

### Command line

```
rsnip                  start the tray app
rsnip --region         start a region capture (uses the running instance if there is one)
rsnip --full           capture all monitors
rsnip --delay N        start a region capture in N seconds
rsnip --text           select an area and copy the text in it
rsnip --last           capture the same area as last time
rsnip --scroll         pick a window, scroll it and capture its whole length
rsnip --record         pick an area and record it (run again to stop)
rsnip --gallery [file] open the library, optionally with a file selected
rsnip --pin <file>      pin an image on top of other windows
rsnip --update         check for a newer version and install it
rsnip --settings       open the Settings window
```

## Troubleshooting

If Print Screen still opens the Windows Snipping Tool, turn off
*Settings → Accessibility → Keyboard → "Use the Print screen key to open screen capture"*.

## Contributing

Contributions are welcome, including AI-assisted ones (every pull request is also reviewed with AI).
See [CONTRIBUTING.md](CONTRIBUTING.md) for building, the test suite and what a pull request needs.

## License

MIT
