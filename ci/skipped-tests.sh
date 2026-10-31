#!/bin/sh
# Lists the tests that skipped themselves on this runner (no sound device, no encoder, no
# GStreamer: see src/testing.rs), in the job's summary and as warnings on the run, so a test that
# never checks anything here doesn't pass unnoticed. `cargo test` writes them to $RSNIP_SKIPS.

set -eu
if [ -z "${RSNIP_SKIPS:-}" ] || [ ! -s "$RSNIP_SKIPS" ]; then
    echo "No test skipped"
    exit 0
fi
echo "Tests that skipped:"
cat "$RSNIP_SKIPS"
{
    echo "### Tests that skipped on this runner"
    echo
    sed 's/^/- /' "$RSNIP_SKIPS"
} >> "${GITHUB_STEP_SUMMARY:-/dev/null}"
while read -r line; do
    echo "::warning::$line"
done < "$RSNIP_SKIPS"
