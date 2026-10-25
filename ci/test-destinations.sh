#!/bin/sh
# Pushes to real servers, in Docker: rclone's S3 and WebDAV servers, an OpenSSH SFTP server, a
# vsftpd FTPS server (with a certificate made here, which the test container trusts) and Samba,
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
mkdir -p "$tmp/s3/shots" "$tmp/dav" "$tmp/sftp" "$tmp/ssh" "$tmp/ftps" "$tmp/tls" "$tmp/smb"
chmod 777 "$tmp/s3" "$tmp/s3/shots" "$tmp/dav" "$tmp/sftp" "$tmp/ftps" "$tmp/tls" "$tmp/smb"
cleanup() {
    docker rm -f "$net-s3" "$net-dav" "$net-sftp" "$net-ftps" "$net-smb" >/dev/null 2>&1 || true
    docker network rm "$net" >/dev/null 2>&1 || true
    # What the servers wrote belongs to their users, so it goes from a container too.
    docker run --rm -v "$(host "$tmp"):/t" --entrypoint rm rclone/rclone -rf /t/s3 /t/dav /t/sftp /t/ftps /t/tls /t/smb >/dev/null 2>&1 || true
    rm -rf "$tmp" 2>/dev/null || true
}
trap cleanup EXIT

# The key the test signs in to the SFTP server with (RSnip uses the user's own).
ssh-keygen -q -t ed25519 -N "" -C rsnip-test -f "$tmp/ssh/id_ed25519"
# The FTPS server's certificate, for its name on the network.
# Readable by the server's user too; it's the container's to change, as it made it.
docker run --rm -v "$(host "$tmp/tls"):/t" --entrypoint sh alpine/openssl -c "openssl req -x509 -newkey rsa:2048 -nodes -days 2 \
    -keyout /t/key.pem -out /t/cert.pem -subj /CN=$net-ftps -addext subjectAltName=DNS:$net-ftps && chmod 644 /t/key.pem" >/dev/null 2>&1

docker network create "$net" >/dev/null
docker run -d --name "$net-s3" --network "$net" -v "$(host "$tmp/s3"):/data" \
    rclone/rclone serve s3 /data --addr :9000 --auth-key rsnip,rsnip-secret >/dev/null
docker run -d --name "$net-dav" --network "$net" -v "$(host "$tmp/dav"):/data" \
    rclone/rclone serve webdav /data --addr :8080 --user rsnip --pass rsnip-secret >/dev/null
docker run -d --name "$net-sftp" --network "$net" -v "$(host "$tmp/ssh/id_ed25519.pub"):/home/rsnip/.ssh/keys/id.pub:ro" \
    -v "$(host "$tmp/sftp"):/home/rsnip/upload" atmoz/sftp rsnip::1000 >/dev/null
docker run -d --name "$net-ftps" --network "$net" -v "$(host "$tmp/tls"):/tls:ro" -v "$(host "$tmp/ftps"):/home/rsnip" \
    -e USERS="rsnip|rsnip-secret|/home/rsnip|1000" -e TLS_CERT=/tls/cert.pem -e TLS_KEY=/tls/key.pem delfer/alpine-ftp-server >/dev/null
docker run -d --name "$net-smb" --network "$net" -v "$(host "$tmp/smb"):/share" \
    dperson/samba -u "rsnip;rsnip-secret" -s "shots;/share;yes;no;no;rsnip" -p >/dev/null

docker run --rm --network "$net" -v "$(host "$PWD"):/work" -w /work -v "$(host "$tmp/ssh"):/keys:ro" \
    -v "$(host "$tmp/s3"):/s3" -v "$(host "$tmp/dav"):/dav" -v "$(host "$tmp/sftp"):/sftp" -v "$(host "$tmp/ftps"):/ftps" -v "$(host "$tmp/smb"):/smb" \
    -v "$(host "$tmp/tls/cert.pem"):/usr/local/share/ca-certificates/rsnip-ftps.crt:ro" -e CARGO_TARGET_DIR=/work/target/destinations \
    -e RSNIP_TEST_S3="http://$net-s3:9000" -e RSNIP_TEST_S3_DIR=/s3 -e RSNIP_TEST_WEBDAV="http://$net-dav:8080" -e RSNIP_TEST_WEBDAV_DIR=/dav \
    -e RSNIP_TEST_SFTP="$net-sftp" -e RSNIP_TEST_SFTP_DIR=/sftp -e RSNIP_TEST_FTPS="$net-ftps" -e RSNIP_TEST_FTPS_DIR=/ftps \
    -e RSNIP_TEST_SMB="//$net-smb/shots" -e RSNIP_TEST_SMB_DIR=/smb \
    "${RSNIP_TEST_IMAGE:-rust:1.97.1}" sh -euc '
        update-ca-certificates >/dev/null
        apt-get update -qq >/dev/null && apt-get install -y -qq smbclient >/dev/null
        # ssh wants the key private, in the home it uses.
        mkdir -p "$HOME/.ssh" && cp /keys/id_ed25519 "$HOME/.ssh/" && chmod 600 "$HOME/.ssh/id_ed25519"
        until ssh-keyscan "$RSNIP_TEST_SFTP" >/dev/null 2>&1; do sleep 1; done
        cargo test real_servers -- --ignored --nocapture'
echo "Pushed to S3, WebDAV, SFTP, FTPS and SMB."
