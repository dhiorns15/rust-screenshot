# Settings

Tray menu → *Settings…* (or the gear in the library, or `rsnip --settings`) opens the Settings window.
Changes apply straight away.

| Section | |
| --- | --- |
| General | **Theme**: System (follows Windows' light or dark mode), Light or Dark. **Start with Windows**, **Open the library when RSnip starts** (otherwise it starts quietly in the tray), **Show notifications**. On Linux: **Keep RSnip in the background** (an icon in the top bar or panel once the library closes) and **Start when you sign in** |
| Capture | **Save screenshots to the folder** (when off, captures go to the clipboard only), **Open the editor after capture** (this saves the capture even if saving is off), **Show the magnifier** (or press M while selecting), **Hide RSnip while capturing** (on by default: the library and Settings windows step aside during a capture or recording and come back after). Every capture is copied to the clipboard. On Linux, only the first two: there the area is picked on a still of the screen |
| Shortcuts | A shortcut for each action (region, all screens, last region, scrolling, record, text). *Change* asks you to press the new combination (Esc cancels); giving it a key another action has moves it. **Also take over Win+Shift+S** for a region capture. It warns if Windows' own Print Screen setting is on or two actions share a key |
| Recording | MP4 or GIF; for MP4s, the frame rate (15, 24, 30 or 60) and codec (H.264 or H.265). The **Sound** card: **Computer sound** (what the computer plays: videos, calls, alerts; on by default) and **Microphone** (off by default), with a drop-down to pick which one, or the default. A microphone that was chosen and isn't connected is shown as "not connected". GIFs have no sound, so the card is greyed out for them. On Linux: **Hide the library while recording**, and what's missing for recording |
| Library & editor | **Save screenshots and recordings to**: RSnip's folder, which the library shows (`RSnip` in your user folder unless you choose another; copies of RSnip from before 0.5.3 keep the Pictures folder's `Screenshots`). After choosing another, Settings offers to move the screenshots and recordings in the previous folder there too, with their edits and search text. On Linux, **Bring in the desktop's screenshots** moves new ones from the desktop's screenshot tool into it; library on the left or along the bottom; **Read text in screenshots for search**; border and drop shadow on exports; **Move old screenshots and recordings to the Recycle Bin** (the Trash on Linux; off by default), after 7, 30 (the default), 90 or 365 days since each was last saved or edited. It runs a minute after RSnip starts and then daily, in the process that pushes automatically, keeps anything an automatic **Push to** destination hasn't received yet, and Settings says how many it would move |
| Push to | The places screenshots and recordings can be sent to: SFTP, WebDAV, S3-compatible storage, SMB shares, FTPS, Discord, Slack, Immich, PhotoPrism and custom uploaders (any upload API; *Add… → Custom uploader from a ShareX file* imports a `.sxcu`). Each is pushed to **Automatically** as it's saved, or **By hand**; *Test* checks it, *Edit* changes it. *Recent pushes* shows the last ten, and why any failed, with a *Retry* for a failure you can push again. Passwords, keys, tokens and webhook addresses are kept in Windows' Credential Manager, or the desktop's keyring on Linux, never in RSnip's settings |
| Updates | **Check automatically** (on by default: shortly after RSnip starts, then daily, it asks GitHub for the latest release; nothing else is sent), **Get release candidates** (off by default: also offers versions to try before they're released), **Install automatically** (off by default; installed copies only: installs a new version as soon as it's found, unless you're in the middle of something), *Check now* |
| System info | What a bug report needs: RSnip's version and install, the OS, CPU, memory, graphics driver, display scale, your settings, the places you push to (their kind and how their last push went, not where they are), the last crash and where files are kept. *Copy* puts it on the clipboard; *Report a problem* (here or in About) opens GitHub's bug form with it filled in, and copies it too. It has no screenshots or file names, and your home folder shows as `~`. `rsnip --system-info` prints it, for when the window won't open |
| About | Version and links |

## Release candidates

A release candidate is a version published for trying before it's released, with a name like `X.Y.Z-rc1`. They're offered only to those who turn on *Settings → Updates → Get release candidates* (off by default); everyone else is offered full releases only.

## Where settings are kept

Settings are stored in `HKCU\Software\RSnip` (on Linux, `~/.config/rsnip/settings.json`).

## Crash reports

If RSnip crashes, it writes what happened to a crash report in `%LOCALAPPDATA%\RSnip\crashes` (on
Linux, `~/.local/state/rsnip/crashes`; the last 10 are kept). Next time, the tray app or the library
says so, and *Settings → System info → Report this crash* opens GitHub's bug form with the report
and System info filled in. A
report says where in RSnip's code it crashed; it has no screenshots or file names. Crashes outside
RSnip's own code, such as in a graphics driver, are reported too.
