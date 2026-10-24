# Linux build and test environment, the same one CI's Linux job uses:
#
#   docker build -t rsnip-test ci
#   docker run --rm -v "$PWD:/work" -w /work -e CARGO_TARGET_DIR=/work/target/linux rsnip-test cargo test
#
# Mesa's lavapipe is a software GPU for the snapshot tests, and DejaVu is the annotation font, so
# rendered snapshots come out the same on every machine. Tesseract and GStreamer are for
# the text-recognition and recording tests, and D-Bus, PipeWire and xdg-desktop-portal for the
# desktop tests (ci/desktop-test.sh).
FROM rust:1.97.1
RUN apt-get update \
    && apt-get install -y --no-install-recommends mesa-vulkan-drivers libvulkan1 fonts-dejavu-core fontconfig tesseract-ocr \
       gstreamer1.0-tools gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-plugins-ugly gstreamer1.0-libav gstreamer1.0-pipewire \
       dbus pipewire pipewire-bin wireplumber xdg-desktop-portal \
    && rm -rf /var/lib/apt/lists/* \
    && rustup component add clippy
