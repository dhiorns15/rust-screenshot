# RSnip

[![CI](https://github.com/dhiorns15/rust-screenshot/actions/workflows/ci.yml/badge.svg)](https://github.com/dhiorns15/rust-screenshot/actions/workflows/ci.yml)

A small, open-source, Snagit-style screenshot tool for Windows and Linux, written in Rust. On
Windows it does everything below; on Linux it is the library and editor for screenshots taken with
your desktop's own tool (see [Linux](#linux)).

- **Takes over Print Screen** (and optionally **Win+Shift+S**) from the built-in Snipping Tool.
- **Freezes the screen** and lets you drag a region, or click a window/monitor to grab it.
- **Copies every capture to the clipboard** and saves a PNG to your Screenshots folder.
- **Remappable hotkey**: pick any key combination from the tray menu.
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

On Linux (x86-64), get `rsnip-vX.Y.Z-linux-x64.tar.gz`, extract it and run `./rsnip`.

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
annotates what's there.

```
cargo build --release
./target/release/rsnip [file]
```

For example, bind a key to `grim -g "$(slurp)" ~/Pictures/Screenshots/shot-$(date +%s).png && rsnip`
to capture an area and open it straight in the editor. Pinning, OCR, copying as files, dragging files
out, and capture/recording are Windows-only. Settings live in `~/.config/rsnip/settings.json`.
When a new version is out, the library's status bar links to it (checked with `curl` each time the
library opens; set `"CheckUpdates": 0` in the settings file to turn that off).

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
| Region capture | Print Screen (or your remapped hotkey), Win+Shift+S, or left-click the tray icon |
| Select | Drag a rectangle, or click to capture the highlighted window. If other windows overlap it, it is brought to the front and captured cleanly |
| One screen | Press Enter or F in the selection overlay to capture the monitor under the mouse |
| All screens | Press A in the selection overlay, or use *Capture all screens* in the tray menu |
| Copy text (OCR) | Tray menu → *Capture text (OCR)*: select an area and its text is copied to the clipboard. In the editor, *Export → Copy text (OCR)* reads the whole image. Uses the OCR built into Windows |
| Pin to screen | Press P in the selection overlay to pin the highlighted area, or use *Export → Pin to screen* in the editor or a thumbnail's right-click menu. The pin floats above other windows: drag to move, mouse wheel to fade, Ctrl+wheel to zoom, right-click to copy or save, double-click or Esc to close |
| Fine-tune | A magnifier next to the cursor shows the pixels under it with their position and colour (M hides it). Arrow keys move the mouse 1 px, Shift+arrows 10 px, including while dragging. C copies the colour under the cursor as #RRGGBB |
| Cancel | Esc or right-click |
| Same area again | Tray menu → *Capture last region*, or R in the selection overlay, captures exactly the area you captured last time |
| Scrolling capture | Tray menu → *Scrolling capture*, then click a window (or drag over the part that scrolls). RSnip scrolls it to the end and stitches one tall image; sticky headers and footers appear once. Esc stops early |
| Record video | Tray menu → *Record screen area*, pick an area, and it records to MP4 (or GIF, via *Record as GIF instead of MP4*), mouse pointer included. A red frame marks the area and a bar shows the time; click *Stop* or press your capture hotkey to finish. The video is saved to the screenshots folder and copied as a file, ready to paste |
| Delayed capture | Tray menu → *Delayed capture* → 3, 5 or 10 seconds, to open a menu or tooltip first. The countdown shows in the tray tooltip |
| Open library | Tray menu → *Open library & editor*, or click the "saved" notification |
| Update | When a new version is out, a notification says so (once per version) and the tray menu offers *Update to RSnip x.y.z…*. If you used the installer, clicking either downloads the new installer, checks its SHA-256 against GitHub's, installs it and restarts RSnip. A standalone `rsnip.exe` opens the download page instead |

Alt+PrtScn and Win+PrtScn are left alone and keep their Windows behaviour.

### Tray menu options

- **Change hotkey…**: press the new combination (e.g. `F8` or `Ctrl+Shift+X`); Esc cancels.
- **Also take over Win+Shift+S**: on by default.
- **Open editor after capture**: every capture opens straight in the editor (it is still copied
  to the clipboard). This saves the capture even if *Save screenshots to folder* is off.
- **Save screenshots to folder**: when off, captures go to the clipboard only.
- **Show notifications**
- **Start with Windows**: adds an entry under `HKCU\...\CurrentVersion\Run`.
- **Open library on launch**: also open the library window when RSnip starts; when off, it starts quietly in the tray.
- **Updates**: *Check now*; *Check automatically* (on by default: shortly after RSnip starts, then
  daily, it asks GitHub for the latest release; nothing else is sent); *Install updates
  automatically* (off by default; installed copies only): installs a new version as soon as it's
  found, unless you're in the middle of something (capturing, recording, a pin or the library open).
  If the library has unsaved edits when an update installs, it asks about them first.

Settings are stored in `HKCU\Software\RSnip`.

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
```

## Troubleshooting

If Print Screen still opens the Windows Snipping Tool, turn off
*Settings → Accessibility → Keyboard → "Use the Print screen key to open screen capture"*.

## Contributing

Contributions are welcome, including AI-assisted ones (every pull request is also reviewed with AI).
See [CONTRIBUTING.md](CONTRIBUTING.md) for building, the test suite and what a pull request needs.

## License

MIT
