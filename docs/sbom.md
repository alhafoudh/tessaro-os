# SBOM and licenses

**Every release carries a bill of materials for each image: what Tessaro
ships and under which license, from the image's packages down to every
crate and npm package.** `mise run sbom:build` writes it from a built image,
and `mise run sbom:check` holds every crate, npm package and vendored file
to a license policy on every push. The tool is `sbom/sbom.rb`, Ruby with
only the standard library, reading what bitbake, cargo and npm already know.

## What is covered, and from where

**Each source answers for what it builds, because no one tool sees all of
it.** Bitbake's SPDX names the image's packages but knows the agent's
crates only as `crate://` downloads without a license, and Webconfig's npm
packages only as one `npmsw://` entry for `bitbake-lock.json`; the clients
are not in the image at all. So each part is read from its own lock file.

| Component | Ecosystem | Read from |
| --- | --- | --- |
| `image` | `yocto` | the image's `tessaro-os-<machine>-<version>.spdx.tar.zst` (`sbom/lib/sbom/yocto.rb`) |
| `tessaro-kiosk` | `cargo` | `cargo metadata` on `agent/`, for the machine's Rust target (`cargo.rb`) |
| `tessaro-ctl` | `cargo` | `cargo metadata` on `agent/`, from the `tessaro-ctl` member only, every platform |
| `tessaro-gui` | `cargo` | `cargo metadata` on `gui/`, from the `tessaro-gui` member only, every platform |
| `try-tessaro` | `cargo` | `cargo metadata` on `gui/`, from the `try-tessaro` member only, every platform |
| `try-tessaro` | `runtime` | the bundled QEMU's `runtime.json`, `brew info --json=v2` of every Homebrew formula it ships plus what `build-qemu-gpu.sh` built or fetched, in the same shape (`runtime.rb`), with `check --runtime` only |
| `tessaro-webconfig` | `npm` | `npm sbom --omit dev --package-lock-only` in `webconfig/` (`npm.rb`) |
| any | `vendored` | `sbom/vendored.yml`, files kept in the repo (the Manrope fonts, presence detection's models, test pictures) |

* **The image's rows are the recipes of what it installs.** Bitbake's
  `create-spdx` is on by default (`INHERIT_DISTRO` in openembedded-core's
  `defaultsetup.conf`), so every image build leaves the tarball in
  `tmp/deploy/images/<machine>/`. `yocto.rb` starts at the image's own
  document and follows `CONTAINS` to each installed package and its
  `GENERATED_FROM` to the recipe. The `-native` recipe documents in the same
  tarball are build tools and are never reached. A row's license is what
  its installed packages declare, not the recipe's `LICENSE`, which also
  covers packages left out of the image.
* **Crates are walked from the workspace members over normal and build
  dependencies**, never dev ones: tests do not ship. For the image the walk
  uses `--filter-platform` with the machine's triple (`Sbom::TARGETS` in
  `sbom/lib/sbom.rb`, keyed like `kas/machine/`), so `windows-sys` and the
  like stay out. The clients ship for every platform and are not filtered.
  The recipe builds every member of `agent/`, so `tessaro-kiosk` starts from
  all of them.
* **npm dev dependencies are left out**: Vite bundles only what the app
  imports, and every build tool is a dev dependency. `npm sbom` reads
  `package-lock.json` alone, so no `node_modules` is needed.
* **A vendored file needs its entry in `sbom/vendored.yml` in the change
  that adds it.** No lock file knows it. An entry whose `path` is gone
  fails the check, so the list does not outlive the file.

**Chromium's own `third_party` is not broken down.** The image row for
`chromium-ozone-wayland` carries the recipe's combined license. Chromium's
`tools/licenses/licenses.py` (what `chrome://credits` shows) could produce
the per-library list from the recipe's source tree; that is not wired in.

## Output

`sbom:build` writes into `build/sbom/<machine>/`, named after the image
(`-dirty` included):

* `<image>.sbom.tar.zst`: a `<image>.sbom/` directory with `yocto/` (the
  image's SPDX 2.2 documents, untouched), `cargo-<component>.spdx.json` and
  `npm-tessaro-webconfig.spdx.json` (SPDX 2.3), and `licenses.csv` and
  `licenses.json`.
* `<image>.licenses.csv`: the same list on its own, the file to read. One
  row per component and package: `component, ecosystem, name, version,
  license, source`. A crate used by several components has a row in each.

It refuses an SPDX tarball from another build than the image beside it (the
`.rootfs.spdx.tar.zst` and `.rootfs.wic.zst` links must name the same
image), and it needs no bitbake: it reads what the image build left.

The cargo documents are written by `spdx.rb`. Their namespace is a hash of
their content, so the same dependencies give the same document;
`SOURCE_DATE_EPOCH` fixes their `created` time when set.

## The policy

**`sbom/licenses.yml` decides which licenses each ecosystem may carry.**
`sbom:check` and `sbom:build` print every row it does not accept and exit 1
on an error.

* **`cargo`, `npm` and `vendored` take an allowlist**, and a miss is an
  error. The binaries and the bundle are Apache-2.0 and ship as one file
  each, so only licenses that ask for no more than a notice are on it (`vendored`
  adds `OFL-1.1` for fonts). An expression passes when one branch of each
  `OR` and every part of each `AND` is allowed (`license.rb`); a missing or
  unreadable license fails.
* **`runtime` takes an allowlist that includes GPL and LGPL**, and a miss
  is an error. It judges the QEMU Try Tessaro bundles, which is GPL, patched
  and built from pinned sources, and ships as separate files in a desktop
  app, its license texts beside it ([try-tessaro.md](try-tessaro.md), "The
  QEMU runtime"). Only a
  Mac that packaged the app has its `runtime.json`, so plain `sbom:check`
  leaves it out and the release workflow's `try` job runs `ruby sbom/sbom.rb check
  --runtime` after `try:build`.
* **`yocto` takes a list to flag**, and a miss is a warning. GPL and LGPL
  are expected in an image, and `INCOMPATIBLE_LICENSE` is the build-time
  gate there (Moonforge's `kas/common/no-gplv3.yml`). What is flagged is
  worth a look before a release: GPLv3 and AGPL, whose terms a locked-down
  kiosk may not meet, and `LicenseRef-*`, licenses SPDX does not know,
  firmware blobs mostly. `known` lists the LicenseRefs read and found
  harmless.
* **A dependency the allowlist refuses gets an `exceptions` entry, with its
  reason, in the change that brings it in.** The entry names the ecosystem,
  the package and the exact expression, so a later change of license is
  judged again. Widening `allow` for one package is the wrong fix.

## Sources

**Every image's GPL, LGPL and AGPL packages have their complete source kept
on the build host**, because distributing an image obliges us to offer it:
the upstream tarball, every patch applied to it and the recipe that builds
it, not a diff. A link to upstream is not enough either: GPL-2.0, the
kernel's, does not allow it for commercial distribution.

* **Bitbake's `archiver` collects them**, set in `tessaro.conf`:
  `ARCHIVER_MODE[src] = "original"` (tarball plus patch series),
  `ARCHIVER_MODE[recipe] = "1"`, target recipes only, licenses matching
  `copyleft_filter`'s default `GPL* LGPL* AGPL*`. They land in
  `tmp/deploy/sources/<TARGET_SYS>/<PF>/`, and `do_deploy_archives` is
  sstate-backed, so they are also in the sstate cache once per architecture.
* **`mise run sources:collect` copies the image's ones into the store**,
  `$TESSARO_SOURCES_DIR` (default `/srv/tessaro/sources`, next to the shared
  cache). Which recipes count comes from the image's SPDX and the
  initramfs's (it ships inside the kernel), the same walk as the license
  list, so stale archives of older builds and recipes the image does not
  install stay out. It fails when an installed copyleft recipe has no
  archive.
* **The store keeps each file once** (`sources.rb`): `files/<name>`, and a
  file of the same name with other content under
  `files/<TARGET_SYS>/<PF>/`. Releases and machines share it, so a release
  only adds what changed; Chromium's tarball alone is several GB.
* **A git source whose bare clone is gone from `cache/downloads/git2/`
  is archived as nothing**, without a warning from bitbake: `do_ar_original`
  only copies a clone that exists, and a shallow `gitshallow_*` tarball does
  not count. `sources:collect` warns about every copyleft recipe it found
  only the recipe of; restore the clone with `git clone --bare --mirror
  <url> cache/downloads/git2/<name>`, then
  `bitbake -f -c ar_original <recipe>` and `-c deploy_archives`.
  `WHOLE_IN_IMAGE` in `sources.rb` names the recipes whose only source is a
  file the image carries as it is.
* **Recipes that build from another recipe's tree** (`libgcc` from
  `gcc-source`, `glibc-locale` from `glibc`, `usbip-tools` from the kernel)
  take that recipe's archive too (`SHARED` in `sources.rb`).
* **Each image gets `<image>.sources.txt`** in `build/sbom/<machine>/`:
  every source file's sha256, size, path in the store and recipe. The
  release carries it, so what belongs to a release stays known after the
  build tree is gone.

**The sources are not published yet.** Before images are distributed to
anyone, the store has to be reachable from where the images are (a
download next to the release, or a bucket the release links to); a GitHub
release takes files of at most 2 GB, which Chromium's tarball exceeds.

## Tasks

* `mise run sbom:build`: the bundle for `$TESSARO_MACHINE`, from its built
  image. The release workflow runs it after each image build and attaches
  the bundle and the license list to the release ([ci.md](ci.md)).
* `mise run sbom:check`: the policy over crates, npm packages and vendored
  files, no image needed. `ci.yml` runs it on every push. It walks the
  agent unfiltered, so it covers every machine's target at once.
* `mise run sources:collect`: the image's copyleft sources into the store
  and `<image>.sources.txt`, from its built image. The release workflow runs
  it after `sbom:build`.
* `mise run sbom:test`: the tool's unit tests, in `sbom/test/`.
