#!/bin/sh
# Installs the Linux packages (installer/linux/build-packages.sh) the way users do, each in a clean
# container of a distribution it's for, and checks that RSnip runs and that what it recommends
# came along: apt on Ubuntu, dnf on Fedora, zypper on openSUSE. Needs Docker.
set -eu
cd "$(dirname "$0")/.."
deb=$(cd target/debian && ls -t *.deb | head -1)
rpm=$(cd target/generate-rpm && ls -t *.rpm | head -1)

# RSnip runs, its app-menu entry is there, and so is GStreamer (with PipeWire) and Tesseract. RSnip
# records a test picture as a recording would be made, with whatever GStreamer pieces came along.
works='rsnip --write-icon /tmp/icon.png && test -s /tmp/icon.png
test -f /usr/share/applications/rsnip.desktop
command -v tesseract
gst-inspect-1.0 --exists pipewiresrc
rsnip --check-recording | tee /tmp/check'
# As H.264 MP4: x264 on Ubuntu, OpenH264 on Fedora (openSUSE's come from Packman, so it's WebM
# there); and it plays back in the library.
h264='grep -q "H.264 MP4" /tmp/check && grep -q "Playback works" /tmp/check'

run() {
    echo "== $1"
    docker run --rm -v "$PWD/target:/target:ro" "$1" sh -euxc "$2"
}

run ubuntu:24.04 "apt-get update -qq
apt-get install -y -qq -o APT::Install-Recommends=true /target/debian/$deb >/dev/null
$works
$h264"
# The container images leave out recommended packages unless asked; a desktop installs them.
run fedora:latest "dnf install -y -q --setopt=install_weak_deps=True /target/generate-rpm/$rpm
$works
$h264"
run opensuse/tumbleweed "zypper --non-interactive -q install --recommends --allow-unsigned-rpm /target/generate-rpm/$rpm
$works"
echo "All packages install and run."
