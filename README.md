# RSnip

[![CI](https://github.com/dhiorns15/rust-screenshot/actions/workflows/ci.yml/badge.svg)](https://github.com/dhiorns15/rust-screenshot/actions/workflows/ci.yml)

A small, open-source, Snagit-style screenshot tool for Windows, written in Rust.

- **Takes over Print Screen** (and optionally **Win+Shift+S**) from the built-in Snipping Tool.
- **Freezes the screen** and lets you drag a region, or click a window/monitor to grab it.
- **Copies every capture to the clipboard** and saves a PNG to your Screenshots folder.
- **Remappable hotkey**: pick any key combination from the tray menu.
- **Library and editor**: browse every screenshot and annotate it with arrows, lines, boxes,
  ellipses, highlighter, blur, pixelate, redact, spotlight, pen, text, numbered steps and crop, with undo/redo. Everything stays
  selectable to move, resize, restyle or delete.

## Download

Get `rsnip.exe` from the [Releases page](https://github.com/dhiorns15/rust-screenshot/releases).
It is a single self-contained exe; put it anywhere and run it.

## Build

Requires the Rust MSVC toolchain.

```
cargo build --release
```

The binary is `target\release\rsnip.exe`.

To publish a release, bump `version` in `Cargo.toml`, commit, then `git tag vX.Y.Z && git push --tags`.
The release workflow runs the tests and attaches the exe and a zip to a new GitHub Release.

## Usage

Run `rsnip.exe`. It sits in the notification area.

| Action | How |
| --- | --- |
| Region capture | Print Screen (or your remapped hotkey), Win+Shift+S, or left-click the tray icon |
| Select | Drag a rectangle, or click to capture the highlighted window. If other windows overlap it, it is brought to the front and captured cleanly |
| One screen | Press Enter or F in the selection overlay to capture the monitor under the mouse |
| All screens | Press A in the selection overlay, or use *Capture all screens* in the tray menu |
| Copy text (OCR) | Tray menu → *Capture text (OCR)*: select an area and its text is copied to the clipboard. In the editor, *Copy text* reads the whole image. Uses the OCR built into Windows |
| Pin to screen | Press P in the selection overlay to pin the highlighted area, or use *Pin* in the editor or a thumbnail's right-click menu. The pin floats above other windows: drag to move, mouse wheel to fade, Ctrl+wheel to zoom, right-click to copy or save, double-click or Esc to close |
| Fine-tune | A magnifier next to the cursor shows the pixels under it with their position and colour (M hides it). Arrow keys move the mouse 1 px, Shift+arrows 10 px, including while dragging. C copies the colour under the cursor as #RRGGBB |
| Cancel | Esc or right-click |
| Delayed capture | Tray menu → *Delayed capture* → 3, 5 or 10 seconds, to open a menu or tooltip first. The countdown shows in the tray tooltip |
| Open library | Tray menu → *Open library & editor*, or click the "saved" notification |

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

Settings are stored in `HKCU\Software\RSnip`.

### Editor

| Tool | Key | |
| --- | --- | --- |
| Select | V | click an annotation to select it; drag to move it, drag its handles to resize it (arrows: drag either end) |
| Arrow | A | drag; hold Shift to snap to 45° |
| Line | L | drag; hold Shift to snap to 45° |
| Box | B | drag |
| Ellipse | E | drag |
| Highlight | H | drag; multiply-blended like a real highlighter |
| Blur | U | drag; strong enough to make text unreadable |
| Pixelate | X | drag; mosaic over sensitive content |
| Redact | R | drag; a solid box (black by default) that hides content for certain |
| Spotlight | S | drag; dims everything outside the area |
| Pen | P | freehand |
| Text | T | click, type, Enter. *Shadow* (on by default) adds a soft drop shadow for readability. Click existing text (or double-click it with any shape tool) to select and edit it; drag text to move it. While selected, resize it with the Size slider or the corner handle, and move it with the grip on its top-left corner. Clear it to delete |
| Step | N | click to drop 1, 2, 3… markers |
| Crop | C | drag to select an area, adjust it by dragging its edges, corners or middle, then click *Crop* (or press Enter, or double-click inside it). *Cancel* or Esc discards it. Undoable |

Whatever you just drew stays selected (except pen strokes), so you can adjust it straight away; the
Select tool picks any annotation. While something is selected, the toolbar's colours, *Size* and
*Shadow* change it, Delete removes it, arrow keys nudge it (Shift: 10 px) and Esc deselects it.
Each move, resize or restyle is one undo step.

Annotations stay editable after saving: the saved PNG is flattened for sharing, and the original
image plus your annotations are kept in a hidden `.rsnip` folder beside it, so reopening the
screenshot lets you keep editing. If the PNG is changed by another program, it opens as a plain image. Editing data for screenshots
deleted outside RSnip is cleaned up automatically.

Ctrl+Z / Ctrl+Y undo/redo, Ctrl+C copies the edited image, Ctrl+S overwrites the file,
Ctrl+Shift+S saves a copy, Ctrl+scroll zooms, Ctrl+0 fits to the window.
The *Export* menu saves a JPG copy, or copies the image *as a file* so it pastes as an attachment into
a folder, chat or email. Its *Border* and *Drop shadow* options are added to every copy, save and
export (the annotations stay editable without them).
Right-click a library thumbnail to open it externally, show it in Explorer, or move it to the Recycle Bin.

### Command line

```
rsnip                  start the tray app
rsnip --region         start a region capture (uses the running instance if there is one)
rsnip --full           capture all monitors
rsnip --delay N        start a region capture in N seconds
rsnip --text           select an area and copy the text in it
rsnip --gallery [file] open the library, optionally with a file selected
rsnip --pin <file>      pin an image on top of other windows
```

## Troubleshooting

If Print Screen still opens the Windows Snipping Tool, turn off
*Settings → Accessibility → Keyboard → "Use the Print screen key to open screen capture"*.

## License

MIT
