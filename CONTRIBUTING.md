# Contributing to RSnip

Thanks for helping! Bug reports, ideas and pull requests are all welcome. For anything bigger than a
small fix, please open an issue first so we can agree on the approach before you spend time on it.
[New issue](https://github.com/dhiorns15/rust-screenshot/issues/new/choose) offers a form for each:
a bug, an idea, or a new place to push to. For a bug, *Settings → System info → Report a problem* in
RSnip copies what the form asks about your system. Security problems go privately, see
[SECURITY.md](SECURITY.md).

## AI-assisted contributions

Using AI tools to write code, tests or docs is welcome. Two things to know:

- **Every pull request is also reviewed with AI**, alongside a human maintainer. AI review comments
  are suggestions; a maintainer makes the call.
- **You own what you submit.** Read and understand every change, be ready to explain why it's
  written the way it is, and run the tests yourself. Pull requests that look generated but unchecked
  (code that doesn't build, tests that don't test the change, invented APIs) will be closed.

Please say in the pull request roughly how much AI helped (the template asks), so reviewers know
what to look at closely.

## Building

RSnip pins its Rust version in `rust-toolchain.toml`; `rustup` picks it up automatically.

- **Windows** (the full app): the MSVC toolchain, then `cargo build`, and `target\debug\rsnip.exe`.
  The library and editor alone: `rsnip --gallery`.
- **Linux** (library and editor only): `cargo build`, or use the same environment CI uses:
  `docker build -t rsnip-test ci`.
- **Installer**: Inno Setup 6, then `powershell -File installer\build.ps1`.
- **Linux packages**: `cargo install cargo-deb cargo-generate-rpm`, `cargo build --release`, then
  `installer/linux/build-packages.sh` writes the `.deb` and `.rpm` (what goes in them, and what they
  depend on, is in `Cargo.toml`). `ci/test-packages.sh` installs them on Ubuntu, Fedora and openSUSE
  in Docker.

Where things are:

| | |
| --- | --- |
| `src/win_main.rs`, `tray.rs`, `hook.rs`, `settings.rs` | the Windows tray app, keyboard hook and settings |
| `src/shortcuts.rs`, `options.rs` | the capture shortcuts and on/off options both the tray app and Settings use |
| `src/capture.rs`, `overlay.rs`, `scroll.rs`, `stitch.rs`, `record.rs`, `encode.rs`, `ocr.rs`, `pin.rs` | capturing, scrolling capture, recording, OCR, pins |
| `src/mp4.rs` | reading recordings' MP4 index, and cutting them without re-encoding (trimming, and seeking on Linux) |
| `src/update.rs` | update checks, release candidates, and installing updates (the installer on Windows, the `.deb`/`.rpm` on Linux) |
| `src/crash.rs` | crash reports: the panic hook, and handlers for native crashes (signals on Linux, unhandled exceptions on Windows) |
| `src/gallery/` | the library window: `mod.rs` (window, toolbar), `library.rs`, `theme.rs`, `color.rs`, `env.rs`, `search.rs` (search by text, app and window), `words.rs` (recognised words and where they are; Tesseract's output on Linux); `background.rs` (Linux's icon in the top bar, `rsnip --tray`, which also records and takes screenshots), `portal.rs`, `recorder.rs` and `pick_area.rs` (recording on Linux: the screen-cast portal, GStreamer, and choosing an area), `screenshot.rs` (Linux's `--region`, `--full`, `--text` and `--last`: the screenshot portal, then the same picker); `player/` (playing and trimming recordings in the library: GStreamer on Linux, Media Foundation on Windows, GIFs in Rust); the Settings window: `settings_window.rs`, and its System info report: `system_info.rs` |
| `src/push/`, `src/http.rs`, `src/secrets.rs` | pushing to other places: the destinations and each kind's requests (`sftp.rs`, `webdav.rs`, `s3.rs`, `smb.rs`, `ftps.rs`, `discord.rs`, `slack.rs`, `immich.rs`, `photoprism.rs`, `custom.rs` with ShareX `.sxcu` import), HTTP for them, and their passwords and keys in the system's credential store; each kind has a section in `docs/destinations.md` (how to set it up, what it needs), which a test checks |
| `src/gallery/pushing.rs`, `auto_push.rs`, `bring_in.rs` | pushing by hand from the library, pushing new items automatically, and (Linux) bringing in the desktop's new screenshots; Settings → Push to is `settings_window/push.rs` |
| `src/gallery/doc.rs` | a screenshot's annotations, undo history and rendering |
| `src/gallery/editor/` | the annotation canvas and tools; `copy_text.rs` is the Copy text tool |
| `src/gallery/platform/` | what differs between Windows and Linux |
| `installer/` | the Inno Setup script and its CI tests; `installer/linux/` has `install.sh` and the `.deb`/`.rpm` files |

## Tests

Run `cargo clippy --all-targets -- -D warnings` and `cargo test` before opening a pull request; CI
runs both on Windows and Linux and must pass. The suite has several layers:

- **Unit and property tests** (`proptest`): random edit sequences against undo/redo, saved layers
  round-tripping, the scrolling-capture stitcher, version parsing, damaged files and settings. If a
  property fails, proptest saves the failing case under `proptest-regressions/`; commit that file so
  the case is replayed from then on.
- **Reference oracles**: `src/gallery/doc/reference.rs` holds slow, obviously-correct versions of
  the image effects; the fast ones must match them exactly. If you optimise an effect, the oracle
  stays as it is.
- **Editor harness** (`src/gallery/editor/tests.rs`): drives the editor with real mouse and keyboard
  events.
- **Window tests** (`src/gallery/app_tests.rs` and `settings_tests.rs`, `egui_kittest`): drive the whole library window
  through its accessibility tree, in a sandbox folder with in-memory settings, so they never touch
  your screenshots or settings. Find controls by their accessible name; give new custom widgets one
  (`WidgetInfo`), which also helps screen readers.
- **Pixel snapshots** (`tests/snapshots/`): the window rendered on a software GPU and compared with
  reference PNGs. They run on Linux only, so everyone gets the same pixels; use the Docker image:

  ```
  docker run --rm -v "$PWD:/work" -w /work -e CARGO_TARGET_DIR=/work/target/linux rsnip-test cargo test
  ```

  After an intended UI change, add `-e UPDATE_SNAPSHOTS=1`, then look at every changed PNG before
  committing it. When a snapshot fails in CI, the run's artifacts include `.new.png` and `.diff.png`.
- **Desktop tests** (`src/gallery/desktop_tests.rs`): Linux recording, screenshots and the
  background icon against real desktop services: xdg-desktop-portal (which checks everything RSnip
  asks of the desktop), PipeWire with WirePlumber, and GStreamer. Only the desktop's own pieces are
  stand-ins: GNOME's screen-cast picker (it shares a test picture through PipeWire) and screenshot
  service (it saves the test picture), the panel the icon goes in, and notifications. `ci/desktop-test.sh` runs them in a session bus and home folder of their own
  (locally, use the Docker image); plain `cargo test` skips them. CI runs them twice: on Debian 13 (PipeWire 1.4,
  the Linux job) and on Ubuntu 24.04 LTS (PipeWire 1.0, `desktop-ubuntu`). Anything RSnip sends the
  desktop belongs here, not only in a unit test.
- **Pushing to other places**: each kind's requests are checked against a pretend server in
  `cargo test` (`http::tests::serve_each`). `ci/test-destinations.sh` also pushes to real S3, WebDAV
  and SFTP servers in Docker (from Git Bash on Windows too); CI's `destinations` job runs it. Nothing
  in the tests uses a real keyring: `src/secrets.rs`'s own test is `#[ignore]`d (run it with
  `cargo test secrets -- --ignored`, on Linux with a keyring unlocked).
- **Panic hunt**: random clicks, drags, keys and undo/redo against the editor. A short run is part of
  `cargo test`; CI's Linux job runs a longer one and prints its seed. Replay a failure with
  `RSNIP_HUNT_SEED=<seed> RSNIP_HUNT_STEPS=<n> cargo test panic_hunt_long -- --ignored`.
- **Installer**: CI builds the installer, installs, upgrades (checking RSnip restarts) and uninstalls
  it; after each release it also updates the previous release with `rsnip --update`.

Some things can only be checked by hand on Windows: the capture overlay, the keyboard hook, tray
menus, scrolling capture, recording and pins. Say in the pull request what you tried.

## UI changes need rendered evidence

A passing build isn't proof that a UI change looks right. Include a before/after image in the pull
request: either the updated snapshots, or a render of the window from a short script:

```
docker run --rm -v "$PWD:/work" -w /work -e CARGO_TARGET_DIR=/work/target/linux \
  -e "RSNIP_UI=size=1280x820 dock=bottom click=Text" -e RSNIP_UI_OUT=target/ui.png \
  rsnip-test cargo test render_window -- --ignored
```

`RSNIP_UI` takes `size=WxH`, `dock=left|bottom`, `theme=light|dark`, `empty`, and `click=<accessible
name>` steps (use `_` for spaces). `render_settings` does the same for the Settings window (`theme=`,
`click=`).

To use the running app from a tool or an AI agent, build with `--features inspect` and run it with
`EGUI_INSPECTION=1`: it then serves its UI tree on `localhost:5719` for clients such as
[egui_mcp](https://github.com/rerun-io/kittest_inspector). Never ship that feature in a release.

## Crash reports

A crash report from a user (see [Settings](docs/settings.md#crash-reports)) says what happened and where. A panic gives its message
and the line of RSnip's code it came from, plus a backtrace: with function names on Linux, but none
in Windows release builds, whose symbols are in a separate `rsnip.pdb`. A native crash (a segfault,
an access violation) has no backtrace. On Windows it lists addresses as `module+offset` instead,
such as `rsnip.exe+0x1A2B3C`: the module tells you whether it was RSnip or, say, a graphics driver.
To find the function behind an `rsnip.exe` address, download `rsnip.exe` and `rsnip.pdb` from the
same release into one folder, open the exe in WinDbg, and run `ln rsnip+0x1A2B3C`.

## Style

- Match the code around you: naming, comment density, idioms. Comments say *why*, not *what*.
- Keep changes focused; one logical change per commit, with a message whose first line says what
  it does (imperative, under ~70 characters) and whose body says why.
- Update the README or the docs for anything a user would notice.
- New behaviour comes with tests at the lowest layer that can show it.

## Releases

Maintainers release by bumping `version` in `Cargo.toml` and pushing a `vX.Y.Z` tag; the release
workflow builds, tests and publishes everything. Its notes list every commit since the previous
release, one line each with a link (`ci/release-notes.sh`), so write commit subjects that make sense
to users reading them there.

To try a release on real machines before users get it, tag a release candidate of the bumped
version first. Builds from the release workflow know their tag (`RSNIP_VERSION`), so a candidate
reports `0.5.3-rc1`, and people who turn on *Settings → Updates → Get release candidates* are
offered it like any update:

1. Bump the version to X.Y.Z, commit, and push a `vX.Y.Z-rc1` tag. The workflow builds and tests it
   as usual and publishes it as a pre-release, with the same downloads. GitHub doesn't count a
   pre-release as the latest release, so only those who opted in are offered it, and the update
   test is skipped.
2. Install it and try what changed, on Windows and on a Linux desktop.
3. If something's wrong, fix it and tag `vX.Y.Z-rc2` (the version stays X.Y.Z). When it's right,
   tag `vX.Y.Z` on the same commit as the last candidate. Once it's published, the workflow deletes
   the candidates' releases, keeping their tags.

A candidate's Linux packages are version `X.Y.Z~rcN`, which apt and rpm put before `X.Y.Z`, so
each candidate and then the release install over the one before (`build-packages.sh` reads the
tag from `RSNIP_VERSION`).

The workflow refuses a tag that doesn't match the version in `Cargo.toml`. A candidate's notes, and
the release's, list everything since the previous release, not since the last candidate.

## License

RSnip is MIT-licensed, and contributions are accepted under the same license.
