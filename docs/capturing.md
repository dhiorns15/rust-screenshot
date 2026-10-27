# Capturing

## Usage

Run `rsnip.exe`. It sits in the notification area.

| Action | How |
| --- | --- |
| Region capture | Print Screen (or the shortcut you chose), Win+Shift+S, or left-click the tray icon |
| Select | Drag a rectangle, or click to capture the highlighted window. If other windows overlap it, it is brought to the front and captured cleanly |
| One screen | Press Enter or F in the selection overlay to capture the monitor under the mouse |
| All screens | Press A in the selection overlay, or use *Capture all screens* in the tray menu |
| Copy text (OCR) | Tray menu → *Capture text (OCR)*: select an area and its text is copied to the clipboard. In the editor, the *Copy text* tool picks out the text in a screenshot to select and copy, and *Export → Copy text (OCR)* copies all of it. Uses the OCR built into Windows |
| Pin to screen | Press P in the selection overlay to pin the highlighted area, or use *Export → Pin to screen* in the editor or a thumbnail's right-click menu. The pin floats above other windows: drag to move, mouse wheel to fade, Ctrl+wheel to zoom, right-click to copy or save, double-click or Esc to close |
| Fine-tune | A magnifier next to the cursor shows the pixels under it with their position and colour (M hides or shows it; Settings → Capture sets whether it starts shown). Arrow keys move the mouse 1 px, Shift+arrows 10 px, including while dragging. C copies the colour under the cursor as #RRGGBB |
| Cancel | Esc or right-click |
| Same area again | Tray menu → *Capture last region*, or R in the selection overlay, captures exactly the area you captured last time |
| Scrolling capture | Tray menu → *Scrolling capture*, then click a window (or drag over the part that scrolls). RSnip scrolls it to the end and stitches one tall image; sticky headers and footers appear once. Esc stops early |
| Record video | Tray menu → *Record screen area*, pick an area, and it records to MP4 (or GIF, chosen in Settings), mouse pointer included. MP4s are H.264 or H.265 at 15, 24, 30 or 60 frames a second (Settings → Recording; H.265 needs an encoder, which most graphics drivers include). A red frame marks the area and a bar shows the time; click *Stop* or press a capture shortcut to finish. The video is saved to RSnip's folder and copied as a file, ready to paste |
| Delayed capture | Tray menu → *Delayed capture* → 3, 5 or 10 seconds, to open a menu or tooltip first. The countdown shows in the tray tooltip |
| Quick settings | Tray menu → *Quick settings…* opens a small window by the tray icon to change what you change most, without opening Settings: whether to save screenshots, open the editor after a capture, show the magnifier and hide RSnip while capturing; and for recordings the format (MP4 or GIF), frame rate, computer sound and microphone. Changes apply at once. *All settings…* opens Settings on Recording. Esc, or clicking elsewhere, closes it |
| Open library | Tray menu → *Open library & editor*, or click the "saved" notification |
| Update | When a new version is out, a notification says so (once per version) and the tray menu offers *Update to RSnip x.y.z…*. If you used the installer, clicking either downloads the new installer, checks its SHA-256 against GitHub's, installs it and restarts RSnip. A standalone `rsnip.exe` opens the download page instead |

Alt+PrtScn and Win+PrtScn are left alone and keep their Windows behaviour.

## Sound in recordings

On Windows, MP4 recordings include whatever the computer is playing, so a video of a demo has its
sound. Turn on *Microphone* as well to add your voice, mixed in with it (the default microphone,
or one you pick). Both switches are in *Settings → Recording*, and in the tray menu's *Quick
settings*. Computer sound is on by default and the microphone is off. GIFs have no sound.

The sound is AAC, 48 kHz stereo. Quiet stretches are recorded as silence, so the sound stays in
step with the picture however long nothing plays.

If the microphone can't be used, RSnip records without it and says so when the recording is
saved. The usual reason is Windows' own privacy setting: under *Settings → Privacy & security →
Microphone*, turn on *Let desktop apps access your microphone*.

## Shortcuts

Giving an action Print Screen is how you choose what Print Screen does, e.g. PrtScn for all screens
and Shift+PrtScn to record. By default PrtScn captures a region and the other actions have no
shortcut. While recording, any capture shortcut stops it.

Which keys do what is set in *Settings → Shortcuts* (see [Settings](settings.md)). Recordings play in the library and can be trimmed there: see [Recordings](editor.md#recordings). On Linux, see [RSnip on Linux](linux.md) for taking screenshots and recording.
