# Places to push to

RSnip can send screenshots and recordings to other places as they're saved, or when you choose.
Set them up in *Settings → Push to* (*Add…*); *Test* checks one without sending anything. Each is
pushed to **Automatically** (whatever's saved from then on, a few seconds after, and again when
edits to it are saved; what was there before isn't) or **By hand**: *Export → Push to* in the
editor and player, or *Push to* in the library's right-click menu, for everything selected. When
the place gives a link, it's copied, and the item's right-click menu has *Copy link* for each place
it went; an edited one pushed again to SFTP, WebDAV, S3, an SMB share or FTPS replaces the file
there, while Dropbox, Discord, Slack, Immich and PhotoPrism get a new copy, and *Copy link* gives the newest. A push that fails by itself is
tried again after a minute, five minutes and half an hour, then reported with a notification.
*Recent pushes*, under the places, lists the last ten pushes, by hand and automatic, with what
went wrong when one didn't, and a *Retry* on the latest failure of a file to a place.

## The places

| Place | What it needs | Link |
|---|---|---|
| SFTP | The server and folder; signs in with your SSH keys through the system's OpenSSH, never a stored password | With a public address for the folder |
| WebDAV | The folder's address, a user name and (app) password | With a public address for the folder |
| S3-compatible | Endpoint, bucket, an access key ID and secret (AWS, R2, B2, Wasabi, MinIO) | With a public address for the bucket |
| SMB share | The share (`\\nas\shots`), and a user and password or none; Samba's `smbclient` on Linux | With a public address for the folder |
| FTPS | The server, user, password and folder; FTP over TLS only, and a NAS's self-signed certificate can be trusted by its fingerprint | With a public address for the folder |
| Dropbox | A free Dropbox app of your own (its App key), and signing in to it from RSnip | A Dropbox link to the file |
| Discord | A channel's webhook address | The attachment |
| Slack | A bot token from a Slack app of your own (`files:write`) and the channel's ID | The file in Slack |
| Immich | The server's address and an API key (`asset.upload`, `user.read`) | The photo in Immich |
| PhotoPrism | The server's address and an app password (`files`, `sessions`, `albums`) | None |
| Custom uploader | Any upload API, described as ShareX's custom uploaders are, or imported from a `.sxcu` file | Where the reply says |

Passwords, keys, tokens and webhook addresses are kept in Windows' Credential Manager, or on Linux the
desktop's keyring (GNOME Keyring, KWallet: any Secret Service), never in RSnip's settings. On Linux
without a keyring, places that need one can't be saved. When the keyring is locked (as it often is
over remote desktop), Settings asks for it to be unlocked when it needs a password, and gives up
after a minute if that isn't answered; automatic pushes never ask. They wait, without using up
their tries, and look again every minute; *Recent pushes* says so, with a *Retry* that asks for the
keyring to be unlocked.
Give RSnip its own key, token or app password where the service has them, with no more than it
needs, so it can be taken back without changing anything else.

**Coming later:** Google Drive, OneDrive, Imgur (with albums) and Box (each needs RSnip
registered with them), Microsoft Teams, Gyazo, Streamable and Synology Photos. Mattermost, Imgbb
and Catbox if someone asks: [suggest a place](https://github.com/dhiorns15/rust-screenshot/issues/new?template=push_destination.yml),
though many hosts with an upload API already work as a custom uploader.

## Setting up each place

How to set up each place RSnip can push screenshots and recordings to (*Settings → Push to →
Add…*), the least each needs (keys, scopes, permissions), what *Test* checks, and what its usual
errors mean. The **?** next to a place in Settings opens its section here.

Where a server drops the connection (a reset, say, or a restart mid-upload) RSnip says *… closed
the connection*: check that it's running and reachable from this computer, and that nothing between
(a firewall, a proxy, a size limit) is turning the upload away.

- [SFTP](#sftp)
- [WebDAV](#webdav)
- [S3-compatible](#s3-compatible)
- [SMB share](#smb-share)
- [FTPS](#ftps)
- [Dropbox](#dropbox)
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

## Dropbox

A folder in your Dropbox, through a free Dropbox app of your own (RSnip has no key of its own to
register, so nothing is shared with anyone else). You make the app once, then sign in to it from
RSnip.

- **Make the app:** at [dropbox.com/developers/apps](https://www.dropbox.com/developers/apps):
  1. *Create app*, then *Scoped access*.
  2. Choose the access: *App folder* (recommended: RSnip can only reach the folder Dropbox makes
     for it, `Apps/<app name>` in your Dropbox) or *Full Dropbox*.
  3. Name the app (any unused name, e.g. `RSnip-<your name>`) and *Create app*.
  4. On the *Permissions* tab, turn on `files.content.write`, `sharing.write` and
     `account_info.read`, then choose *Submit*. (Changing permissions later means signing in
     again.)
  5. On the *Settings* tab, copy the **App key**. The app stays in *Development* status, which is
     fine for one account (yours); nothing needs to be applied for.
- **Set up in RSnip:** *Settings → Push to → Add… → Dropbox*:
  1. Paste the **App key**, and give the **Folder** (`/Screenshots`; in the app's folder with *App
     folder* access, so the files land in `Apps/<app name>/Screenshots`). Each Dropbox place is one
     folder; add more places for more folders.
  2. Choose *Sign in to Dropbox*. Dropbox opens in your browser: allow the app, and it shows a
     code. Paste the code into *Code from Dropbox* and choose *Finish signing in*, then *Save*.
  3. *Share link* (on by default) makes a Dropbox link to each file pushed, and copies it.
- **Signs in with:** OAuth with PKCE, as desktop apps do, so there's no app secret to keep. RSnip
  keeps only the refresh token Dropbox gives, in the credential store with the other secrets; it
  trades it for a short-lived access token at each push, and *Remove* forgets it. To take access
  back, remove the app under *Settings → Connected apps* on dropbox.com.
- **Needs:** the three permissions above (`files.content.write` to upload, `sharing.write` for the
  link, `account_info.read` for *Test*); with *Share link* off, `sharing.write` isn't used.
- **Test:** signs in and asks whose Dropbox it is (*Connected as …*), changing nothing.
- **Files:** one by that name already in the folder isn't replaced: Dropbox adds a number to the
  new one (`Screenshot (1).png`), and the link is to that. Over 150 MB (recordings) goes up in 8 MB
  pieces.
- **Errors:** *Dropbox signed RSnip out: sign in again in Settings → Push to*: the app was removed
  from your account, or its permissions changed; choose *Edit → Sign in to Dropbox*. *The Dropbox
  app needs the … permission*: turn it on under *Permissions*, choose *Submit*, and sign in again.
  *Your Dropbox is full*. *Dropbox doesn't accept that folder or file name*: check the folder.
  *Dropbox says RSnip is sending too many requests*: wait a little. *Dropbox didn't accept that
  code*: a code works once and expires, so choose *Sign in to Dropbox* again for a new one.
  *Dropbox doesn't know this app key*: check it on the app's *Settings* tab. *The Dropbox app can't
  make links* (from Test connection): turn on `sharing.write`, or turn off Share link; a push that
  can't make its link still puts the file up, without a link.
- **Link:** a Dropbox link to the file (it opens a preview page; add `dl=1` to the end to download).

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
