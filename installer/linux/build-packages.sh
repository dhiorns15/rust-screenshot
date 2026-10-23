#!/bin/sh
# Builds RSnip's Linux packages from a release build (`cargo build --release` first):
# target/debian/rsnip_<version>-1_amd64.deb and target/generate-rpm/rsnip-<version>-1.x86_64.rpm.
# Needs cargo-deb and cargo-generate-rpm (`cargo install cargo-deb cargo-generate-rpm`); what goes
# in them is set in Cargo.toml.
set -eu
cd "$(dirname "$0")/../.."
mkdir -p target/linux-package
target/release/rsnip --write-icon target/linux-package/rsnip.png
# Not stripped: function names stay, for crash reports' backtraces.
cargo deb --no-build --no-strip
cargo generate-rpm
ls -l target/debian/*.deb target/generate-rpm/*.rpm
