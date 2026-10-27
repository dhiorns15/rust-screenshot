# Command line

The installer doesn't put RSnip on the PATH, so on Windows give its full path, e.g. in PowerShell
`& "$env:LOCALAPPDATA\Programs\RSnip\rsnip.exe" --system-info` (or `rsnip.exe` in the folder the
zip was unpacked to).

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
rsnip --quick-settings open the quick settings, as the tray menu does
rsnip --system-info    print what Settings → System info shows, for a bug report
```

On Linux:

```
rsnip [file]           open the library, optionally with a file selected
rsnip --region         pick an area of the screen and capture it
rsnip --full           capture the whole screen
rsnip --delay N        pick an area to capture in N seconds
rsnip --text           select an area and copy the text in it (needs Tesseract)
rsnip --last           capture the same area as last time
rsnip --gallery [file] open the library, optionally with a file selected
rsnip --update         check for a newer version and install it (.deb and .rpm installs)
rsnip --settings       open the Settings window
rsnip --quick-settings open the quick settings, as the icon's menu does
rsnip --record         pick an area and record it (run again to stop)
rsnip --record-screen  pick a screen or window and record it (run again to stop)
rsnip --tray           put RSnip's icon in the top bar or panel (the library does this itself)
rsnip --tray-quit      remove it again
rsnip --system-info    print what Settings → System info shows, for a bug report
```
