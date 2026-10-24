#!/bin/sh
# Builds RSnip's Linux packages from a release build (`cargo build --release` first):
# target/debian/rsnip_<version>-1_amd64.deb and target/generate-rpm/rsnip-<version>-1.x86_64.rpm.
# Needs cargo-deb and cargo-generate-rpm (`cargo install cargo-deb cargo-generate-rpm`); what goes
# in them is set in Cargo.toml.
set -eu
cd "$(dirname "$0")/../.."
mkdir -p target/linux-package
target/release/rsnip --write-icon target/linux-package/rsnip.png
# A release candidate (RSNIP_VERSION v0.5.3-rc2, as the release workflow sets it) is packaged as
# 0.5.3~rc2: apt and rpm put "~" before the release, so each candidate and then the release
# upgrade the one before. Its files are named 0.5.3-rc2, as GitHub renames ones with a "~".
version=$(tr -d '\r' <Cargo.toml | sed -n 's/^version = "\(.*\)"/\1/p' | head -1)
name=$version
case "${RSNIP_VERSION:-}" in
    v"$version"-rc*) name=${RSNIP_VERSION#v}; version=$version~${name#"$version"-} ;;
esac
# Not stripped: function names stay, for crash reports' backtraces.
cargo deb --no-build --no-strip --deb-version "$version-1" -o "target/debian/rsnip_${name}-1_amd64.deb"
cargo generate-rpm -s "version = \"$version\"" -o "target/generate-rpm/rsnip-${name}-1.x86_64.rpm"
ls -l target/debian/*.deb target/generate-rpm/*.rpm
