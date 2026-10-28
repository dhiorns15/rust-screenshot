# Capturing

## Usage

Run `rsnip.exe`. It sits in the notification area.

| Action | How |
| --- | --- |
| Region capture | Print Screen (or the shortcut you chose), Win+Shift+S, or *Region* in the capture panel |
| Select | Drag a rectangle, or click to capture the highlighted window. If other windows overlap it, it is brought to the front and captured cleanly |
| One screen | Press Enter or F in the selection overlay to capture the monitor under the mouse |
| All screens | Press A in the selection overlay, or use *Capture all screens* in the tray menu |
| Copy text (OCR) | Tray menu → *Capture text (OCR)*: select an area and its text is copied to the clipboard. In the editor, the *Copy text* tool picks out the text in a screenshot to select and copy, and *Export → Copy text (OCR)* copies all of it. Uses the OCR built into Windows |
| Pin to screen | Press P in the selection overlay to pin the highlighted area, or use *Export → Pin to screen* in the editor or a thumbnail's right-click menu. The pin floats above other windows: drag to move, mouse wheel to fade, Ctrl+wheel to zoom, right-click to copy or save, double-click or Esc to close |
| Fine-tune | A magnifier next to the cursor shows the pixels under it with their position and colour (M hides or shows it; Settings → Capture sets whether it starts shown). Arrow keys move the mouse 1 px, Shift+arrows 10 px, including while dragging. C copies the colour under the cursor as #RRGGBB |
| Cancel | Esc or right-click |
| Same area again | Tray menu → *Capture last region*, or R in the selection overlay, captures exactly the area you captured last time |
| Scrolling capture | Tray menu → *Scrolling capture*, then click a window (or drag over the part that scrolls). RSnip scrolls it to the end and stitches one tall image; sticky headers and footers appear once. Esc stops early. On Linux it works on an X11 desktop: *Scrolling capture…* in the icon's menu, *Scrolling* in the capture panel, or `rsnip --scroll`, then drag over the part that scrolls (see [RSnip on Linux](linux.md#scrolling-capture)) |
| Record video | Tray menu → *Record screen area*, pick an area, and it records to MP4 (or GIF, chosen in Settings), mouse pointer included. MP4s are H.264 or H.265 at 15, 24, 30 or 60 frames a second (Settings → Recording; H.265 needs an encoder, which most graphics drivers include). A red frame marks the area and a bar shows the time; click *Stop* or press a capture shortcut to finish. The video is saved to RSnip's folder and copied as a file, ready to paste |
| Delayed capture | Tray menu → *Delayed capture* → 3, 5 or 10 seconds, to open a menu or tooltip first. The countdown shows in the tray tooltip |
| Capture panel | Left-click the tray icon (or tray menu → *Capture panel…*) to open a panel by it: **Image** and **Video** tabs down the left, what to capture or record and the options for it beside them, and a round red button (*Capture*, or *Record*) with the action's shortcut under it. Image: *Selection* is Region, Full screen, Last area, Scrolling or Text (copy the text in an area), and the switches are Open in editor, Save to folder, Magnifier and Hide RSnip while capturing. Video: *Selection* is Area, the format (MP4 or GIF), frame rate, computer sound, microphone and camera. The switches are the same settings as in Settings and change at once; the tab and the selections are remembered. Enter does the same as the button, and the panel closes first so it is not in the picture. *Open library & editor* opens the library, and the gear opens Settings (on Recording, from the Video tab). Drag it by any empty part to move it: it opens where you left it from then on. Esc, its ×, or clicking elsewhere closes it; with the pin (*Keep open*) on, clicking elsewhere leaves it open, and it steps aside while you capture or record and comes back after |
| Open library | Tray menu → *Open library & editor*, or click the "saved" notification |
| Update | When a new version is out, a notification says so (once per version) and the tray menu offers *Update to RSnip x.y.z…*. If you used the installer, clicking either downloads the new installer, checks its SHA-256 against GitHub's, installs it and restarts RSnip. A standalone `rsnip.exe` opens the download page instead |

Alt+PrtScn and Win+PrtScn are left alone and keep their Windows behaviour.

## Sound in recordings

On Windows, MP4 recordings include whatever the computer is playing, so a video of a demo has its
sound. Turn on *Microphone* as well to add your voice, mixed in with it (the default microphone,
or one you pick). Both switches are in *Settings → Recording*, and on the capture panel's Video
tab. Computer sound is on by default and the microphone is off. GIFs have no sound.

The computer's sound comes from the default output unless you pick another under *Computer
sound*. Pick one if recordings come out silent or quiet: with a mixer such as SteelSeries Sonar,
Voicemeeter or Elgato Wave Link, apps play to the mixer's own channels (Sonar's Media, Game and
Chat, say), and only the headphones or speakers it plays to have them all. If the output you
picked isn't connected, RSnip records the default one and says so. The outputs and microphones
are listed again each time you open one of the lists, so one plugged in meanwhile is there.

On Windows the sound is AAC, 48 kHz stereo. Quiet stretches are recorded as silence, so the sound
stays in step with the picture however long nothing plays. On Linux it's AAC in MP4s and Opus in
WebMs, and comes from the desktop's sound server: see [Sound](linux.md#sound) in RSnip on Linux.

If the microphone can't be used, RSnip records without it and says so when the recording is
saved. The usual reason is Windows' own privacy setting: under *Settings → Privacy & security →
Microphone*, turn on *Let desktop apps access your microphone*.

## Camera in recordings

Turn on *Camera* in *Settings → Recording*, or on the capture panel's Video tab, to show your
camera while you record: a round picture of you (mirrored, as in a video call) in a window of its
own, on top of everything else. Nothing is added to the video afterwards: the bubble is recorded
because it's on the screen, wherever you put it in the area, so GIFs have it too. It's off by
default. Pick which camera under the switch (the first one unless you choose); the list is read
again each time you open it, and a camera you picked that isn't connected is shown as "not
connected" (the first one is shown instead, and the "Recording saved" notification says so).

To set it up before recording, open the capture panel on Video with Camera on: the bubble shows
beside it. Hover over it for its buttons: three sizes (small, medium and large) and × to close it.
Drag it to where you want it in your recordings. RSnip remembers the size and the place, and the
bubble closes again when the panel does, or you switch to Image or turn Camera off. `rsnip
--camera` shows it by itself the same way, without the panel.

The bubble opens when the recording starts and closes when it stops. It goes where you last
dragged it if that's inside what you're recording (always, for a whole screen), and otherwise at
the bottom right of the area. You can drag it while recording too. It shows no buttons then, since
they'd be in the video: turn the mouse wheel over it for a bigger or smaller one. Clicking or
dragging it never takes the focus from the app you're recording.

A camera's picture takes a moment to reach the screen (often a tenth of a second or more), so on
Windows RSnip times it and holds the microphone back by as much, keeping your voice in step with
your face. The computer's sound isn't held back: it goes with what's on the screen.

If the camera can't be used (another app has it, or it's unplugged), the recording goes on without
it and the "Recording saved" notification says why. On Windows the usual reason is the privacy
setting: under *Settings → Privacy & security → Camera*, turn on *Let desktop apps access your
camera*. On Linux, see [Camera](linux.md#camera) in RSnip on Linux.

## Shortcuts

Giving an action Print Screen is how you choose what Print Screen does, e.g. PrtScn for all screens
and Shift+PrtScn to record. By default PrtScn captures a region and the other actions have no
shortcut. While recording, any capture shortcut stops it.

Which keys do what is set in *Settings → Shortcuts* (see [Settings](settings.md)). Recordings play in the library and can be trimmed there: see [Recordings](editor.md#recordings). On Linux, see [RSnip on Linux](linux.md) for taking screenshots and recording, and
[its keyboard shortcuts](linux.md#keyboard-shortcuts), which GNOME runs.
