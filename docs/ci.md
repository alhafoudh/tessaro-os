# Continuous integration

GitHub Actions, in `.github/workflows/`. Each workflow calls the mise tasks
rather than repeating what they run, so a job does what the same task does
on a workstation, and changing a task changes CI with it.

## Every push and pull request: `ci.yml`

**Everything that builds without bitbake is checked on every push and pull
request**, on GitHub's own runners so it never waits behind an image build:

* `agent`: `agent:lint` and `agent:test`.
* `ctl`: `ctl:build`, uploaded as the `tessaro-ctl-linux-x86_64` artifact.
  A job of its own so the release build runs next to the agent's tests, not
  after them.
* `gui`: `gui:lint`, `gui:test`, and `gui:build`, uploaded as
  `tessaro-gui-linux-x86_64`.
* `webconfig`: `webconfig:setup`, `webconfig:lint`, `webconfig:test` (which
  includes `bitbake-lock.json` being current) and `webconfig:build`, then
  `player:test`, the player page's unit tests, which need the same Node.
* `sbom`: `sbom:test` and `sbom:check`, the license policy over every crate,
  npm package and vendored file ([sbom.md](sbom.md)). It builds nothing:
  cargo and npm read the lock files, and the runner's own ruby runs the
  tool.

`jdx/mise-action` installs only the toolchain a job needs from `mise.toml`,
so CI runs the pinned Rust and Node; `MISE_AUTO_INSTALL=false` stops
`mise run` from installing the rest. `Swatinem/rust-cache` caches the cargo
target dirs that `mise.toml` sets (`build/cargo-target`,
`build/gui-target`). A newer push to the same ref cancels the older run.

`agent:integration` is not in CI: it runs the agent until Ctrl-C and asserts
nothing.

## The image jobs: `image.yml`

**Images are built and tested only in a run someone starts by hand**, and
both workflows that do it, `release.yml` and `e2e.yml`, call `image.yml`
(`workflow_call`) for those jobs, so building and testing an image is
written once. Its inputs are the machines (a JSON array), whether e2e runs,
the arguments for `e2e:run` and whether the images are for a release. Its
jobs show up inside the caller's run, as `image / build (<machine>)` and
`image / e2e`.

* `version` names the run's version with `image:name` (`<version>-<sha>`),
  the one every image carries.
* `build` runs once per machine, one at a time, on the self-hosted runner:
  `image:name`, then `image:build`, then `sbom:build`, which fails the leg
  on a license the policy does not allow, then `sources:collect`, which
  copies the image's copyleft sources into the store on the host and fails
  when one has no archive. It uploads the versioned
  `tessaro-os-<machine>-<version>-<sha>.wic.zst`, its `.wic.bmap`, its
  `.sbom.tar.zst`, its `.licenses.csv` and its `.sources.txt` as separate,
  unzipped artifacts (`archive: false`), each named after its file. Both
  run here because they read what the image build left in the tree; the
  sources themselves stay in the store ([sbom.md](sbom.md), "Sources").
* `e2e` runs after every build leg, on the self-hosted runner, when e2e is
  asked for and qemux86-64 was built: `e2e:setup` and `e2e:run`, then
  `build/e2e/` uploaded as `e2e-logs`.

**`image.yml` has no concurrency of its own; the job that calls it does.**
Both callers put that job in the `beef-yocto` group (**e2e is a job of its
own** below). The group cannot sit at the top of `image.yml` as well: a
called workflow in the same group as its caller waits for itself forever.

## By hand: `e2e.yml`

**The qemu image of a branch, built and tested, without clients or a
release**, from the Actions tab: pick the branch in "Use workflow from" (a
pull request's branch is tested this way) and, optionally, the arguments for
`e2e:run`. It calls `image.yml` with qemux86-64 and e2e. Pull requests do
not run it on their own, and a fork's branch cannot be picked: the jobs run
on the build host (**Never trigger the image workflows from
`pull_request`** below). It shows in the Actions tab once it is on the
default branch; from then on any branch can be picked.

## By hand: `release.yml`

**Releases are built only when someone starts the workflow**, from the
Actions tab ("Run workflow"). Its inputs pick the machines, whether e2e
runs on the qemu image, the arguments for `e2e:run` (`-o '--tag
~reboot'`), and whether the run becomes a GitHub release. That last one
starts unticked, so a run is a dev build unless it is ticked; without it the
images and clients stay workflow artifacts.

* `pick` turns the picked machines into the JSON array for `image.yml` and
  names the run's version the way `image.yml`'s `version` does: the clients
  need it before `image.yml` has finished, the only point its outputs
  exist.
* `image` calls `image.yml` with the picked machines and e2e.
* `clients` runs `ctl:build` and `gui:build` on GitHub's runners, one leg
  per platform: linux-x86_64, linux-aarch64, macos-arm64 and
  windows-x86_64. It uploads `tessaro-ctl-<version>-<sha>-<platform>` and
  `tessaro-gui-<version>-<sha>-<platform>`, as `.tar.gz`, or `.zip` on
  Windows and for the macOS `Tessaro.app`, as `clients-<platform>`.
* `try` builds Try Tessaro ([try-tessaro.md](try-tessaro.md)) when the try
  box is ticked, which needs genericarm64 picked too (`pick` fails up
  front otherwise). It waits for `image`, all of it, since a job cannot
  wait for one build leg, so the package carries this run's image. A
  failed e2e or another machine's failed leg does not stop it. It is
  a matrix, one leg per platform on GitHub's runners: each row names the
  runner, the machine whose image it bundles and the package, and macOS
  (`macos-arm64`, genericarm64, a DMG) is the only one; another platform is
  a row there and its packaging in `try:build`. A leg runs `try:build`
  around the image `build` uploaded (on macOS after `brew install glib
  pixman libslirp dtc pkgconf`, with `build/qemu-gpu` cached under the hash
  of `build-qemu-gpu.sh`, so QEMU is rebuilt only when a pin moved),
  checks the bundled runtime's licenses (`sbom.rb check --runtime`), and
  uploads `try-tessaro-<version>-<sha>-<platform>.<package>` unzipped
  (`archive: false`, as `build` uploads the images), so it downloads as the
  `.dmg` itself, and beside it the bundled QEMU's source,
  `try-tessaro-<version>-<platform>-qemu-source.tar` (`qemu-source.sh`, see
  **The QEMU runtime** in [try-tessaro.md](try-tessaro.md)), which
  `release` attaches with it. A run with release unticked is how to get a
  DMG without publishing anything.
* `release` runs only when asked and only when every job passed, e2e
  included, while an unticked `try` is skipped. A skipped e2e would let
  `image` pass, so `pick` refuses a release without e2e ticked.
  It tags the built commit `v<version>-<sha>` and attaches every image,
  bmap, SBOM bundle, license list, client archive and Try Tessaro package.
  `pick` fails up front when release is ticked without e2e or without
  qemux86-64, or when a release of the same version (the part before the
  sha) already exists, so no run builds for hours toward a release it
  cannot make. Every release therefore needs a new `DISTRO_VERSION` in
  `meta-tessaro-distro/conf/distro/tessaro.conf`.

**The self-hosted runner builds images and nothing else.** The clients
take minutes on GitHub's runners and need none of the build host's cache,
so they run there, alongside the image builds, and never queue behind one.
They skip lint and test: `ci.yml` ran those on the push.

**Webconfig is not released on its own.** The image builds it with bitbake
and the device serves it; served from anywhere else it cannot reach a
device, whose API refuses browser requests from any other origin
(**Trust and auth** in [api.md](api.md)).

**The Linux clients are built on Ubuntu 22.04**, so they run on glibc 2.35
and newer. They link the system's openssl (libssl3), and `tessaro-gui` loads
X11 or Wayland and xkbcommon at runtime.

**The macOS clients are signed ad hoc only**, the way `gui/package-macos.sh`
signs `Tessaro.app`. A download carries the quarantine flag, so Gatekeeper
refuses to open it until that is cleared:

```sh
xattr -dr com.apple.quarantine Tessaro.app tessaro-ctl "Try Tessaro.app"
```

**On Windows the tasks run in Git Bash.** `MISE_WINDOWS_DEFAULT_INLINE_SHELL_ARGS`
makes mise hand their sh bodies to `bash -c` instead of cmd.

**e2e is a job of its own, on the build host, in the workspace the build
left.** The suite boots runqemu out of the kas build tree that built the
image (native qemu, OVMF, the `qemuboot.conf`), so it needs that tree, not
a downloaded `.wic`. There is one runner and its workspace persists, so the
tree is still there; a failed e2e is re-run on its own ("Re-run failed
jobs") without building again. `needs: build` waits for every build leg, so
on a run with several machines e2e starts after the last one.

* **The job that calls `image.yml` is in the `beef-yocto` concurrency
  group, in `release.yml` and `e2e.yml` alike**, so no other run builds
  into the workspace between `build` and `e2e`. A run started while one is
  going waits; GitHub keeps only one run waiting in the group and cancels an
  older waiting one, whichever of the workflows it came from.
* **`e2e` first checks the tree holds this run's image**: `image:name` must
  be this run's version, and the deploy directory's
  `tessaro-os-qemux86-64.rootfs.wic.zst` must point at that name. A failed
  qemu build fails here instead of testing an older image.
* `E2E_WORKER_OFFSET=10` moves its ports away from a workstation's own VMs
  and `dev:tunnel`'s forwards on the same host. Its gems go under `build/`.
* **`e2e` runs 8 VMs at a time (`E2E_JOBS`), so the longest lane sets its
  wall time.** parallel_tests hands out whole files and each file boots its
  own VM, so a lane that outgrows the others is split into files at a case
  boundary its order allows (`agent_browser` and `agent_systemd` are one
  such split). The job log's per-case timestamps show which lane that is.

**A release comes only from a clean tree.** `build` fails up front when
`release` is set and `image:name` ends in `-dirty`, since that name would
not match any commit.

## The self-hosted runner

**Image jobs run on the build host and use its cache in place.** The shared
download and sstate cache runs to tens of gigabytes; storing and restoring it through
`actions/cache` or S3 on every run would cost more than the build it saves.
On the host it is a directory that is simply there.

* **`clean: false` on checkout.** The default `git clean -ffdx` would wipe
  the kas layer checkouts and `build/`, and every build would start from
  nothing. The runner's workspace persists between runs like a checkout on a
  workstation.
* **The cache lives outside every checkout, in `/srv/tessaro/cache`**, so
  deleting or recloning a checkout never loses it, and the workstation
  checkout and the runner's share it. A checkout's `cache` is a symlink to
  it (`ln -s /srv/tessaro/cache cache`); a worktree's links to its main
  checkout's (CLAUDE.md), which resolves to the same place. In CI the job
  makes the link from `TESSARO_CACHE_DIR`, a repository variable (Settings,
  Secrets and variables, Actions, Variables) set to that path. `/cache` is
  gitignored, so the link does not make the image `-dirty`. The runner runs
  as the user who builds on the host, so both write it with the same
  permissions.
* **The images' copyleft sources go to `/srv/tessaro/sources`**, a store
  shared the same way, which `sources:collect` reads from
  `TESSARO_SOURCES_DIR`, another repository variable.
* **One build at a time.** The matrix has `max-parallel: 1`, and each job
  waits while `pgrep -af 'kas-container|bitbake'` finds anything, so a
  workstation build or e2e run on the host finishes first. A job cancelled
  mid-build can leave its kas container running; check `docker ps` before
  the next one.
* **Never trigger the image workflows from `pull_request`**: not
  `release.yml`, not `e2e.yml`, and never call `image.yml` from a workflow
  that `pull_request` triggers. The repository is public and a self-hosted
  runner runs whatever the workflow checks out, a fork's pull request
  included; `workflow_dispatch` keeps that to people with write access to
  the repository.

### Setting it up

**`.github/setup-runner.sh` sets the runner up**, run on the build host as
the user who builds there, from a checkout. Running it again skips what is
done and refreshes the service's `PATH`.

```sh
.github/setup-runner.sh --repo OWNER/NAME --token TOKEN \
    [--work /big/disk/runner-work] [--cache /path/to/cache] [--sources /path/to/sources]
.github/setup-runner.sh --repo OWNER/NAME --remove --token TOKEN
```

The token comes from the repository's Settings, Actions, Runners, "New
self-hosted runner" (for `--remove`, the runner's "Remove" button); with the
`gh` CLI logged in as a repository admin it can be left out. The script:

1. **Checks the host for what a build and e2e need** (DEVELOPMENT.md,
   **Prerequisites**): `docker` and `kvm` group membership, `/dev/kvm`,
   `kas-container`, `mise`, `zstd`, and `ruby` with `bundle`, whose Bundler
   must match `test/e2e/Gemfile.lock`.
2. **Downloads the latest runner** into `~/actions-runner` (`--dir`) and runs
   its `installdependencies.sh`.
3. **Registers it with the label `yocto`**. Its workspace holds a checkout
   with a `build/<machine>/` for every machine built, so `--work` should
   point at a disk with room for them.
4. **Writes the runner's `.env`** with a `PATH` holding the directories of
   those tools. The systemd service does not read the login shell's profile,
   and without them every job fails on a missing `mise`.
5. **Installs and starts the service** as that user (`svc.sh`).
6. **Sets the `TESSARO_CACHE_DIR` and `TESSARO_SOURCES_DIR` variables**
   through `gh`, or prints what to set by hand. The defaults are
   `/srv/tessaro/cache` (`--cache`) and `/srv/tessaro/sources`
   (`--sources`), which it creates.

A new build host gets the shared cache first, as the user who builds there:

```sh
sudo mkdir -p /srv/tessaro && sudo chown "$USER": /srv/tessaro
mv cache /srv/tessaro/cache 2>/dev/null || mkdir -p /srv/tessaro/cache
ln -s /srv/tessaro/cache cache    # in the checkout
```
