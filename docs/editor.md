# The editor, library and Gallery

## The toolbar

The toolbar has the tools in the middle, each named under its icon, grouped as selecting, drawing,
text and marking up, hiding, and cropping (hover one for its key and how to use it). Undo and redo
are on the left, and *Copy*, *Save* and the *Export* menu on the right. In a narrow window the names
are left out, and the actions move to a row of their own. The row below shows only the options for the current tool or selected
annotation (colour, size, shadow, delete) and a hint on how to use it.

## Tools

| Tool | Key | |
| --- | --- | --- |
| Select | V | click an annotation to select it; drag to move it, drag its handles to resize it (arrows: drag either end) |
| Copy text | O | reads the text in the screenshot and picks it out. Drag across words to select them (double-click selects a line, Ctrl+A everything), then Ctrl+C or *Copy selection*; *Copy all text* copies the lot. Blurred, pixelated and redacted text stays hidden |
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
| Redact | R | drag; a solid box (black by default) that hides content for certain. *Auto-redact* reads the text in the screenshot and covers every email address, IP address, API key or token, card number and phone number it finds, in one undo step (it needs text reading, like Copy text) |
| Crop | C | drag to select an area, adjust it by dragging its edges, corners or middle, then click *Crop* (or press Enter, or double-click inside it). *Cancel* or Esc discards it. Undoable |

## Editing

Whatever you just drew stays selected (except pen strokes), so you can adjust it straight away; the
Select tool picks any annotation. While something is selected, the options row's colours, *Size* and
*Shadow* change it, Delete removes it, arrow keys nudge it (Shift: 10 px) and Esc deselects it.
Each move, resize or restyle is one undo step.

## Annotations stay editable

Annotations stay editable after saving: the saved PNG is flattened for sharing, and the original
image plus your annotations are kept in a hidden `.rsnip` folder beside it, so reopening the
screenshot lets you keep editing. If the PNG is changed by another program, it opens as a plain image. Editing data for screenshots
deleted outside RSnip is cleaned up automatically.

## Keys and Export

Ctrl+Z / Ctrl+Y undo/redo, Ctrl+C copies the edited image (or the selected text, with *Copy text*), Ctrl+S overwrites the file,
Ctrl+Shift+S saves a copy, Ctrl+scroll zooms, Ctrl+0 fits to the window.

The *Export* menu saves a copy (PNG or JPG), copies the image *as a file* so it pastes as an
attachment into a folder, chat or email, copies all the text in it (OCR), or pins it to the screen. Its
*Border* and *Drop shadow* options are added to every copy, save and export (the annotations stay
editable without them).

## The library

The library sits on the left, or along the bottom as a filmstrip so the editor gets the full width:
switch with the two layout buttons next to *Library*. RSnip remembers the choice.

Search finds screenshots by the words in them, the app and window they were captured from, and their name
(every word must match something, in any order). RSnip reads the text with Windows' built-in text
recognition (Tesseract on Linux), in the background, and keeps it beside the screenshots in the hidden
`.rsnip` folder; nothing leaves your computer. If you turn on *Settings → Library & editor →
Descriptions*, search also looks in an AI model's description of each screenshot (what app or site it
shows and what's happening in it, with a few keywords), so "bar chart" or "error dialog" can find a
screenshot with no such words in it; see [Descriptions](settings.md#descriptions). Hover a result to see
where it came from and the line that matched. On Linux without Tesseract, search uses names only.

The library groups screenshots by date (Today, Yesterday, This week, …), each heading saying how many
it has. Click a heading to fold that date away, and again to bring it back; RSnip remembers which
are folded (Today always starts unfolded), and a search shows what matches even in a folded date.

Ctrl+click and Shift+click select several, to copy them as files, delete them, pin them or drag them
out together.

Recordings appear with an MP4/GIF badge and play in the library (below).

Drag a library thumbnail into Explorer, a chat app or an email to drop the file there.

Right-click a library thumbnail to open it externally, show it in Explorer, pin it, or move it to the Recycle Bin.

## The Gallery

*Gallery*, at the left of the toolbar, swaps the editor and the library for a grid of large
thumbnails, grouped by date like the library (and folded the same way) so you can look through
everything and choose. *Sort* orders them by date, name, size or type, and the button beside it
reverses the order (newest, largest and A–Z are the usual ones to start from); by date and by type
they're grouped under headings, by name and size they're one run. *Editor* brings the editor back;
RSnip remembers which page you were on, and how you sorted it. A click
selects (Ctrl+click and Shift+click select several), and a double click or Enter opens it in the
editor, or in the player for a recording. The arrow keys move the selection, Ctrl+A selects everything
shown and Esc clears it. Search, dragging a thumbnail out, the right-click menu and the buttons for
several selected ones work as in the library.

In a window at least 900 pixels wide the gallery has an info panel on the right (drag its edge to
resize it). For the selected screenshot or recording it shows a larger preview, its name, kind,
dimensions (and length, for a recording), file size, date and time, the app and window it was
captured from, where it has been pushed (with *Copy link* where the place gives one), whether it
has annotations you can still edit, and the start of the text read from it (*Copy text* copies all
of it), with *Open*, *Show in folder*, *Copy as file* and *Delete*. With several selected it adds up
their size and offers the same bulk actions as the toolbar.

## Recordings

Recordings play in the library: click one for play and pause (Space), a timeline to click or drag,
and ←/→ to skip 5 seconds. A recording with sound plays it in step with the picture; the speaker
button beside play mutes it (and unmutes it) without stopping the picture, and the player remembers
that for the next recording. If the sound can't be played, "No sound" shows beside the time, and
pointing at it says why. *Trim* marks the part to keep, with the timeline's handles or *Start here*
/ *End here*; while trimming, playing stops at the end of the part kept and plays it again from its
start. *Save trimmed copy* saves it beside the original as "… (trimmed)". MP4s are cut without re-encoding, so
it's instant and loses nothing, but the cut starts on the keyframe at or before the start
(recordings have one every second); GIFs are cut to the frame. A recording with sound keeps the
sound of the part kept, in step with the picture, and the library's details say "with sound" beside
its length. On Linux, playing MP4s needs GStreamer's H.264 and AAC decoders (`gstreamer1.0-libav` on
Debian and Ubuntu, which the `.deb` recommends). For a recording, *Copy* copies the file, to paste into a folder, chat or email,
and *Export* saves a copy to another folder or shows it in its folder, selected (as
*Show in folder* does for screenshots, and in the library's right-click menu).
