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
| `try-tessaro` | `runtime` | the bundled QEMU's `runtime.json`, `brew info --json=v2` of every formula it ships (`runtime.rb`), with `check --runtime` only |
| `tessaro-webconfig` | `npm` | `npm sbom --omit dev --package-lock-only` in `webconfig/` (`npm.rb`) |
| any | `vendored` | `sbom/vendored.yml`, files kept in the repo (the Manrope fonts) |

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
  error. The binaries and the bundle are MIT and ship as one file each, so
  only licenses that ask for no more than a notice are on it (`vendored`
  adds `OFL-1.1` for fonts). An expression passes when one branch of each
  `OR` and every part of each `AND` is allowed (`license.rb`); a missing or
  unreadable license fails.
* **`runtime` takes an allowlist that includes GPL and LGPL**, and a miss
  is an error. It judges the QEMU Try Tessaro bundles, which is GPL and
  ships unmodified as separate files in a desktop app, its license texts
  beside it ([try-tessaro.md](try-tessaro.md), "The QEMU runtime"). Only a
  Mac that packaged the app has its `runtime.json`, so plain `sbom:check`
  leaves it out and the release job runs `ruby sbom/sbom.rb check
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

## Distributing images

**An SBOM lists licenses; it does not meet them.** Shipping an image to
someone else brings the GPL and LGPL obligation to offer the corresponding
source. Bitbake's `archiver` class (`INHERIT += "archiver"`,
`ARCHIVER_MODE[src] = "original"`) collects it per recipe into
`tmp/deploy/sources/`; it is not enabled.

## Tasks

* `mise run sbom:build`: the bundle for `$TESSARO_MACHINE`, from its built
  image. The release workflow runs it after each image build and attaches
  the bundle and the license list to the release ([ci.md](ci.md)).
* `mise run sbom:check`: the policy over crates, npm packages and vendored
  files, no image needed. `ci.yml` runs it on every push. It walks the
  agent unfiltered, so it covers every machine's target at once.
* `mise run sbom:test`: the tool's unit tests, in `sbom/test/`.
