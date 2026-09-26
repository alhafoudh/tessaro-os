# File storage

**`tessaro-ctl files upload|download|sync|list|move|rm` manages `/data/files`,
which nginx serves read-only at `http://127.0.0.1/files/`**, so a site can
keep its videos, images and JSON on the device and have them with the
network down. Remote paths are from the store's root, a leading `/`
optional. `files list [REMOTE]` is `ls -l`: one level, directories with a
trailing `/`, `-R` for the whole tree (`files-list` has `recursive`, which
sync and download set). `files move SOURCE... DEST` is `mv`: into DEST when
it is a directory or ends in `/`, a rename otherwise, within the store so
atomic. `files sync LOCAL_DIR [REMOTE_DIR]` is rsync `-r --delete`
compared on size and mtime (whole seconds) alone: new and changed files are
sent, whatever the local directory lacks is removed, after a prompt unless
`-y`, and `--dry-run` only prints the plan. Symlinks and special files on the
client are skipped with a warning. The logic is `agent/tessaro-agent/src/files.rs`
and `agent/tessaro-ctl/src/files.rs`; path rules are `agent/protocol/src/files.rs`,
shared so both ends refuse the same paths.

* **A file arrives like an image upload.** `files-begin` describes it (path,
  size, mtime), `files-chunk`s of up to `UPDATE_CHUNK` append it to
  `/data/tessaro/files-upload/upload.part`, each synced before it is
  acknowledged, and the last one sets the mtime, mode 0644, and renames it
  into place. So nginx never serves half a file, and the staging being on the
  same filesystem is what makes the rename atomic. One upload slot: the same
  path, size and mtime resumes, across an agent restart too; anything else
  drops the unfinished one. A file the store already has with the same size
  and mtime is answered as complete and never sent.
* **The agent stores a file of its own the same way**, whole: `Files::store`
  writes `/data/tessaro/files-store.part`, syncs it and renames it in, under
  the same lock and space reserve as uploads but outside the upload slot. The
  one user is the `audio test --input` recording (see [audio.md](audio.md)).
* **Downloads are request/response, not a stream**: `GET files/content` of
  up to `UPDATE_CHUNK` raw bytes from an offset, the whole file's size and
  mtime in its headers ([api.md](api.md)), written to a `.part` beside the
  target and renamed, with the device's mtime.
* **256 MiB of `/data` is always left free** (`RESERVE`): the Chromium
  profile, the settings and an image update's staging live there too.
* **Nothing follows a symlink**, on either side of nginx: the agent checks
  every component with `symlink_metadata` and opens with `O_NOFOLLOW`, and the
  location has `disable_symlinks on`. The agent never makes one; one made by
  hand could otherwise reach `/data/tessaro` and its tokens.
* **The nginx location is in the self-test's server block**
  (`10-tessaro-selftest.conf`), because that is the one server on
  `127.0.0.1:80`. `autoindex off`, `Access-Control-Allow-Origin: *` and
  `Cache-Control: no-cache` - restated there, since a location's `add_header`
  replaces the server's.
* **An https kiosk site may use it without a prompt.** `http://127.0.0.1` is
  potentially trustworthy, so it is not mixed content, but Chromium's Local
  Network Access (139 on) puts a public site's requests to the loopback behind
  a permission prompt. `LocalNetworkAccessAllowedForUrls` lists the device
  origins, rendered by the agent with the serial and HID grants
  (`ORIGIN_POLICIES` in `render.rs`), so it follows `browser.url`.
* **A factory reset empties it; unclaim keeps it** - it is the site's
  content, not access to the device. The reset renames the store to
  `/data/tessaro/files-trash`, makes it again empty and then deletes the trash,
  so what nginx serves is gone at once, and the boot oneshot finishes a
  deletion that was cut short. `--wipe-data` and `--repartition` re-create
  `/data` anyway. `/data/files` itself comes from tmpfiles (0755 root) and the
  agent makes it too.
