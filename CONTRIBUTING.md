# Contributing to RSnip

Thanks for helping! Bug reports, ideas and pull requests are all welcome. For anything bigger than a
small fix, please open an issue first so we can agree on the approach before you spend time on it.

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

Where things are:

| | |
| --- | --- |
| `src/win_main.rs`, `tray.rs`, `hook.rs`, `settings.rs` | the Windows tray app, keyboard hook and settings |
| `src/shortcuts.rs`, `options.rs` | the capture shortcuts and on/off options both the tray app and Settings use |
| `src/capture.rs`, `overlay.rs`, `scroll.rs`, `stitch.rs`, `record.rs`, `encode.rs`, `ocr.rs`, `pin.rs` | capturing, scrolling capture, recording, OCR, pins |
| `src/update.rs` | update checks and installing updates |
| `src/crash.rs` | crash reports: the panic hook, and handlers for native crashes (signals on Linux, unhandled exceptions on Windows) |
| `src/gallery/` | the library window: `mod.rs` (window, toolbar), `library.rs`, `theme.rs`, `color.rs`, `env.rs`, `search.rs` (search by text, app and window); the Settings window: `settings_window.rs`, and its System info report: `system_info.rs` |
| `src/gallery/doc.rs` | a screenshot's annotations, undo history and rendering |
| `src/gallery/editor/` | the annotation canvas and tools |
| `src/gallery/platform/` | what differs between Windows and Linux |
| `installer/` | the Inno Setup script and its CI tests |

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

A crash report from a user (see the README) says what happened and where. A panic gives its message
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
- Update the README for anything a user would notice.
- New behaviour comes with tests at the lowest layer that can show it.

## Releases

Maintainers release by bumping `version` in `Cargo.toml` and pushing a `vX.Y.Z` tag; the release
workflow builds, tests and publishes everything. Its notes list every commit since the previous
release, one line each with a link (`ci/release-notes.sh`), so write commit subjects that make sense
to users reading them there.

## License

RSnip is MIT-licensed, and contributions are accepted under the same license.
