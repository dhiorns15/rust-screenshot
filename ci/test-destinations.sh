#!/bin/sh
# Pushes to real servers, in Docker: rclone's S3 and WebDAV servers and an OpenSSH SFTP server,
# on a network of their own, then runs src/push's real_servers test against them from a Rust
# container (the image CI's Linux job uses, or RSNIP_TEST_IMAGE). Each server keeps what it's sent
# in a folder the test can look in. Needs Docker; works from Git Bash on Windows too.
set -eu
cd "$(dirname "$0")/.."
# Docker on Windows wants D:/... paths, not Git Bash's /d/...
export MSYS_NO_PATHCONV=1
host() { cygpath -m "$1" 2>/dev/null || echo "$1"; }

net="rsnip-destinations-$$"
tmp=$(mktemp -d)
# The S3 server's bucket "shots" is a folder in its own.
mkdir -p "$tmp/s3/shots" "$tmp/dav" "$tmp/sftp" "$tmp/ssh"
chmod 777 "$tmp/s3" "$tmp/s3/shots" "$tmp/dav" "$tmp/sftp"
cleanup() {
    docker rm -f "$net-s3" "$net-dav" "$net-sftp" >/dev/null 2>&1 || true
    docker network rm "$net" >/dev/null 2>&1 || true
    rm -rf "$tmp"
}
trap cleanup EXIT

# The key the test signs in to the SFTP server with (RSnip uses the user's own).
ssh-keygen -q -t ed25519 -N "" -C rsnip-test -f "$tmp/ssh/id_ed25519"

docker network create "$net" >/dev/null
docker run -d --name "$net-s3" --network "$net" -v "$(host "$tmp/s3"):/data" \
    rclone/rclone serve s3 /data --addr :9000 --auth-key rsnip,rsnip-secret >/dev/null
docker run -d --name "$net-dav" --network "$net" -v "$(host "$tmp/dav"):/data" \
    rclone/rclone serve webdav /data --addr :8080 --user rsnip --pass rsnip-secret >/dev/null
docker run -d --name "$net-sftp" --network "$net" -v "$(host "$tmp/ssh/id_ed25519.pub"):/home/rsnip/.ssh/keys/id.pub:ro" \
    -v "$(host "$tmp/sftp"):/home/rsnip/upload" atmoz/sftp rsnip::1000 >/dev/null

docker run --rm --network "$net" -v "$(host "$PWD"):/work" -w /work -v "$(host "$tmp/ssh"):/keys:ro" \
    -v "$(host "$tmp/s3"):/s3" -v "$(host "$tmp/dav"):/dav" -v "$(host "$tmp/sftp"):/sftp" -e CARGO_TARGET_DIR=/work/target/destinations \
    -e RSNIP_TEST_S3="http://$net-s3:9000" -e RSNIP_TEST_S3_DIR=/s3 -e RSNIP_TEST_WEBDAV="http://$net-dav:8080" -e RSNIP_TEST_WEBDAV_DIR=/dav \
    -e RSNIP_TEST_SFTP="$net-sftp" -e RSNIP_TEST_SFTP_DIR=/sftp "${RSNIP_TEST_IMAGE:-rust:1.97.1}" sh -euc '
        # ssh wants the key private, in the home it uses.
        mkdir -p "$HOME/.ssh" && cp /keys/id_ed25519 "$HOME/.ssh/" && chmod 600 "$HOME/.ssh/id_ed25519"
        until ssh-keyscan "$RSNIP_TEST_SFTP" >/dev/null 2>&1; do sleep 1; done
        cargo test real_servers -- --ignored --nocapture'
echo "Pushed to S3, WebDAV and SFTP."
