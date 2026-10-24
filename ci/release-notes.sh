#!/bin/sh
# Release notes for tag $1: one line per change since the previous release, each linked to its
# commit (or pull request, when the subject names one), and a link to the full comparison.
#
#   ci/release-notes.sh v0.4.0 > notes.md
#
# Needs the whole history (actions/checkout with fetch-depth: 0). GITHUB_REPOSITORY is set on CI.
# Release candidates (vX.Y.Z-rcN) don't count as previous releases: a release, and each of its
# candidates, lists everything since the release before.
set -eu
tag="$1"
repo="${GITHUB_REPOSITORY:-dhiorns15/rust-screenshot}"
url="https://github.com/$repo"
previous=$(git describe --tags --abbrev=0 --exclude '*-*' "$tag^" 2>/dev/null || true)
range="${previous:+$previous..}$tag"

# Skip merges and the version-bump commits; a "(#123)" in a subject already links its pull request.
changes=$(git log --no-merges --reverse --format='%H %s' "$range" |
    grep -vE '^[0-9a-f]+ Release [0-9]+\.[0-9]+\.[0-9]+$' |
    while read -r sha subject; do
        case "$subject" in
            *"(#"[0-9]*")") echo "- $subject" ;;
            *) echo "- $subject ([${sha%"${sha#???????}"}]($url/commit/$sha))" ;;
        esac
    done)

echo "## What's changed"
echo
echo "${changes:-- No changes besides the version number.}"
if [ -n "$previous" ]; then
    echo
    echo "**Full changelog**: $url/compare/$previous...$tag"
fi
