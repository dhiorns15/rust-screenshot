# Linux build and test environment, the same one CI's Linux job uses:
#
#   docker build -t rsnip-test ci
#   docker run --rm -v "$PWD:/work" -w /work -e CARGO_TARGET_DIR=/work/target/linux rsnip-test cargo test
#
# Mesa's lavapipe is a software GPU for the snapshot tests, and DejaVu is the annotation font, so
# rendered snapshots come out the same on every machine. Tesseract is for the text-recognition test.
FROM rust:1.97.1
RUN apt-get update \
    && apt-get install -y --no-install-recommends mesa-vulkan-drivers libvulkan1 fonts-dejavu-core fontconfig tesseract-ocr \
    && rm -rf /var/lib/apt/lists/* \
    && rustup component add clippy
