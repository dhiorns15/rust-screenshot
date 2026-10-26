# Building

## On Windows

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

## On Linux

To build it from source on Linux:

```
# Needs Rust (rustup.rs) and a C compiler, e.g. build-essential or gcc.
cargo build --release
./target/release/rsnip [file]
```

## Tests

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

## Releases

To publish a release, bump `version` in `Cargo.toml`, commit, then `git tag vX.Y.Z && git push --tags`.
The release workflow runs the tests and attaches the Windows installer, exe and zip, and a Linux
build, to a new GitHub Release. It then installs the previous release on a fresh machine and checks
that `rsnip --update` updates it to the new one. To try a release before it goes out, tag a release
candidate first, `vX.Y.Z-rc1`: it's published as a pre-release, which the update check offers only
to those who turned on *Get release candidates* (see [CONTRIBUTING.md](../CONTRIBUTING.md#releases)).
