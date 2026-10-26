# Places to push to

How to set up each place RSnip can push screenshots and recordings to (*Settings → Push to →
Add…*), the least each needs, what *Test* checks, and what its usual errors mean. The **?** next to
a place in Settings opens its section here.

Passwords, keys, tokens and webhook addresses are kept in Windows' Credential Manager, or on Linux
the desktop's keyring (GNOME Keyring, KWallet: any Secret Service), never in RSnip's settings.
Give RSnip its own key, token or app password where the service has them, with no more than it
needs, so it can be taken back without changing anything else.

Where a server drops the connection (a reset, say, or a restart mid-upload) RSnip says *… closed
the connection*: check that it's running and reachable from this computer, and that nothing between
(a firewall, a proxy, a size limit) is turning the upload away.

- [SFTP](#sftp)
- [WebDAV](#webdav)
- [S3-compatible](#s3-compatible)
- [SMB share](#smb-share)
- [FTPS](#ftps)
- [Discord](#discord)
- [Slack](#slack)
- [Immich](#immich)
- [PhotoPrism](#photoprism)
- [Custom uploader](#custom-uploader)

## SFTP

A folder on a server you can reach over SSH: a NAS, a VPS, a web server.

- **Set up:** the server, its port (22) and the folder (from your home folder, or `/srv/…`); a user
  name unless `~/.ssh/config` gives one.
- **Signs in with:** your SSH keys and ssh-agent, through the system's OpenSSH (built into Windows
  10 and 11, *Settings → System → Optional features → OpenSSH Client*; `openssh-client` on Linux).
  RSnip never asks for or keeps an SSH password: add your public key to the server
  (`ssh-copy-id user@server`) and check that `ssh user@server` signs in without asking.
- **Needs:** that user may write to the folder, and make folders in it if it isn't there yet.
- **Test:** signs in and goes to the folder, changing nothing. A server it hasn't met before is
  remembered (in `~/.ssh/known_hosts`); one whose key has changed is refused.
- **Errors:** *The server didn't accept your SSH keys*: the key isn't on the server, or ssh-agent
  doesn't have it. *The server's host key has changed*: if you expected that (a reinstall), remove
  the old one with `ssh-keygen -R <server>`.
- **Link:** with *Link to share* set (where the folder is served on the web), that plus the file's
  name.

## WebDAV

A folder on a WebDAV server: Nextcloud, ownCloud, a NAS's WebDAV service, Koofr, pCloud.

- **Set up:** the folder's address (Nextcloud:
  `https://<server>/remote.php/dav/files/<user>/<folder>`), the user name and password.
- **Needs:** write access to the folder, and to make it if it isn't there. Where the server has app
  passwords (Nextcloud: *Settings → Security → Devices & sessions*), make one for RSnip; with
  two-factor sign-in, an app password is the only way.
- **Test:** asks for the folder (`PROPFIND`), changing nothing; a folder that isn't there yet is
  made with the first push.
- **Errors:** *The server didn't accept the user name or password*: check them, or use an app
  password.
- **Link:** with *Link to share* set, that plus the file's name.

## S3-compatible

A bucket on AWS S3, Cloudflare R2, Backblaze B2, Wasabi, MinIO or anything else that speaks S3.

- **Set up:** the endpoint (`https://s3.<region>.amazonaws.com`,
  `https://<account>.r2.cloudflarestorage.com`, B2's or Wasabi's S3 endpoint, MinIO's address), the
  bucket, an optional folder in it, and an access key ID and secret. The region is taken from the
  endpoint (`auto` for R2) unless you give one.
- **Needs:** a key for this bucket only, that may list it (for *Test*) and put objects in it:
  - AWS: an IAM user or role with `s3:ListBucket` on the bucket and `s3:PutObject` on its objects:
    ```json
    {
      "Version": "2012-10-17",
      "Statement": [
        { "Effect": "Allow", "Action": "s3:ListBucket", "Resource": "arn:aws:s3:::<bucket>" },
        { "Effect": "Allow", "Action": "s3:PutObject", "Resource": "arn:aws:s3:::<bucket>/*" }
      ]
    }
    ```
  - Backblaze B2: an application key with *Read and Write* access to the bucket only.
  - Cloudflare R2: an API token with *Object Read & Write*, for the bucket only.
  - Wasabi, MinIO: a key with a policy like AWS's.
- **Test:** asks for the bucket (`HEAD`), changing nothing.
- **Errors:** *Access denied*: the key ID or secret is wrong, or the key may not use this bucket.
  *There's no bucket named …*: check its name. *… is in another region*: give the bucket's region.
- **Link:** with *Link to share* set (the bucket's public address or a CDN in front of it), that
  plus the file's key.

## SMB share

A folder on a Windows share, a Samba server or a NAS.

- **Set up:** the share (`\\nas\shots` or `//nas/shots`), an optional folder in it, and a user name
  and password, or none.
- **Signs in:** on Windows, with Windows' own file sharing: as you are signed in to Windows when no
  user is given, else as that user until you sign out. On Linux, with Samba's `smbclient` (the
  `smbclient` package; `samba-client` on Fedora and openSUSE), as a guest when no user is given.
- **Needs:** that user may write to the share (share and folder permissions both: *Change* or
  *Full control*), and make the folder if it isn't there.
- **Test:** connects and looks for the folder, changing nothing.
- **Errors:** *The server didn't accept the user name or password*. *The server has no share by
  that name*. *Windows is already connected to … as someone else*: Windows keeps one sign-in per
  server; leave the user empty to use that one, or disconnect it (`net use \\nas /delete`). *The
  share didn't let RSnip in, or write there*: check the permissions. *Lost touch with the server*
  (Windows): the server went away mid-connection, or can't be reached: check that it's on.
  Automatic pushes try again after a minute, 5 minutes and 30 minutes.
- **Link:** with *Link to share* set, that plus the file's name.

## FTPS

A folder on an FTP server, over TLS (a NAS, most web hosts).

- **Set up:** the server, its port (21 for explicit TLS, the usual; 990 for implicit), the user
  name, password and folder.
- **TLS:** always; plain FTP, which sends the password unencrypted, isn't offered. File transfers
  reuse the sign-in's TLS session, as vsftpd and ProFTPD (most NASes) require.
- **Certificate:** one from an authority the computer trusts is used as it is. For a self-signed
  one, which most NASes have, choose *Trust the server's certificate…* in the form: RSnip shows its
  SHA-256 fingerprint; check it's the one your server's settings show (on a Synology, *Control
  Panel → Security → Certificate*), then *Trust it*. From then on only that certificate is accepted,
  whatever its name or issuer; *Stop trusting it* goes back to the computer's authorities.
- **Needs:** that user may write to the folder, and make it if it isn't there.
- **Test:** signs in and goes to the folder, changing nothing.
- **Errors:** *The server didn't accept the user name or password*. *The server doesn't offer FTP
  over TLS*: turn on TLS (FTPS) on the server. *The server's certificate isn't one this computer
  trusts*: trust it as above, after checking its fingerprint. *The server's certificate has changed
  since you trusted it*: if you renewed it, trust the new one; if not, something may be between you
  and the server.
- **Link:** with *Link to share* set, that plus the file's name.

## Discord

A channel, through a webhook.

- **Set up:** in Discord, *Server Settings → Integrations → Webhooks → New Webhook*, choose the
  channel, *Copy Webhook URL*, and paste it into RSnip.
- **Needs:** *Manage Webhooks* on the server, to make the webhook; the webhook itself may only post
  to its channel. Anyone with its address can post there, so it's kept with the secrets.
- **Test:** asks Discord for the webhook, posting nothing.
- **Errors:** *Discord doesn't know this webhook*: it was deleted; make another. *Too big for
  Discord*: files are limited to 10 MB without a server boost.
- **Link:** the file Discord keeps.

## Slack

A channel, as a Slack app of your own.

- **Set up:** at [api.slack.com/apps](https://api.slack.com/apps), *Create New App → From a
  manifest*, pick the workspace and paste:
  ```yaml
  display_information:
    name: RSnip
  features:
    bot_user:
      display_name: RSnip
  oauth_config:
    scopes:
      bot:
        - files:write
  ```
  Then *Install to Workspace*, copy the *Bot User OAuth Token* (`xoxb-…`) from *OAuth &
  Permissions*, and invite the app to the channel (`/invite @RSnip`). RSnip needs the channel's ID
  (in the channel's details, at the bottom, like `C0123ABCDEF`), not its name.
- **Needs:** the `files:write` scope only.
- **Test:** checks the token (`auth.test`) and says whose it is, posting nothing.
- **Errors:** *Slack didn't accept the bot token*. *The Slack app needs the files:write scope*: add
  it, then reinstall the app. *The app isn't in that channel*: invite it. *Slack can't find that
  channel*: use its ID.
- **Link:** the file in Slack.

## Immich

Your Immich server's library.

- **Set up:** the server's address and an API key: *Account Settings → API Keys → New API Key*.
- **Needs:** `asset.upload` (to push) and `user.read` (for *Test*); nothing else. Without
  `user.read` pushing still works, but *Test* fails. On a server others use too, consider a user of
  RSnip's own, so its uploads are easy to tell apart.
- **Test:** asks who the key belongs to (`/api/users/me`).
- **Errors:** *Immich didn't accept the API key*. *This API key may not upload*: give it
  `asset.upload`.
- **Link:** the photo or video in Immich (it opens with your browser's Immich sign-in).

## PhotoPrism

Your PhotoPrism server's library, as its own web upload adds photos.

- **Set up:** the server's address, an app password, and an optional album (by its title; made if
  it isn't there).
- **App password:** *Settings → Account → Apps and Devices*, or on the server
  `photoprism auth add -n RSnip -s "files sessions albums" <user>`.
- **Needs:** the `files` and `sessions` scopes, and `albums` to put photos in an album (without it
  they're imported but not put in one); or full access.
- **Test:** asks whose the app password is.
- **Errors:** *PhotoPrism didn't accept the app password*. *The app password needs the files and
  sessions scopes*.
- **Link:** none: PhotoPrism doesn't say which photo an upload became.

## Custom uploader

Any service with an HTTP upload API, described as ShareX's custom uploaders are: image hosts,
self-hosted ones like Zipline, XBackBone or Chevereto, and your own.

- **Set up:** *Add… → Custom uploader from a ShareX file (.sxcu)* fills it in from the file the
  service gives you; its key or token is kept as the secret. Or *Add… → Custom uploader* and fill it
  in:
  - **Upload address** and **method** (POST, PUT or PATCH).
  - **File field:** the form field the file goes in; empty to send the file as the whole body.
  - **Form fields** and **headers**, one `Name: value` a line.
  - **Link:** where the link is in the reply: `{json:data.url}` (a path into JSON, like
    `files[0].url`), `{response}` (the whole reply) or `{header:Location}`.
  - **Token:** optional; `{secret}` is replaced by it wherever it's written (`Authorization: Bearer
    {secret}`). `{filename}` is replaced by the file's name, and `{base64:…}` by what's in it,
    encoded (for `Basic` sign-ins).
- **Needs:** whatever the service's upload needs; a token that may only upload, where it has them.
- **Test:** sends a request without a file and says what the server replied; push something to be
  sure.
- **Errors:** what the server replied, as it said it. *RSnip doesn't fill in {…}*: the `.sxcu` uses
  something RSnip doesn't have yet, like `{regex:…}`.
- **Link:** where the reply says.
