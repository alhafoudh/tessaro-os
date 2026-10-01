# Continuous integration

GitHub Actions, in `.github/workflows/`. Each workflow calls the mise tasks
rather than repeating what they run, so a job does what the same task does
on a workstation, and changing a task changes CI with it.

## Every push: `ci.yml`

**Everything that builds without bitbake is checked on every push and pull
request**, on GitHub's own runners so it never waits behind an image build:

* `agent`: `agent:lint`, `agent:test`, and `ctl:build`, uploaded as the
  `tessaro-ctl-linux-x86_64` artifact.
* `gui`: `gui:lint`, `gui:test`, and `gui:build`, uploaded as
  `tessaro-gui-linux-x86_64`.
* `webconfig`: `webconfig:setup`, `webconfig:lint`, `webconfig:test` (which
  includes `bitbake-lock.json` being current) and `webconfig:build`.

`jdx/mise-action` installs only the toolchain a job needs from `mise.toml`,
so CI runs the pinned Rust and Node. `Swatinem/rust-cache` caches the cargo
target dirs that `mise.toml` sets (`build/cargo-target`,
`build/gui-target`). A newer push to the same ref cancels the older run.

`agent:integration` is not in CI: it runs the agent until Ctrl-C and asserts
nothing.

## By hand: `image.yml`

**Images are built only when someone starts the workflow**, from the Actions
tab ("Run workflow"). Its inputs pick the machines, whether e2e runs on the
qemu image, the arguments for `e2e:run` (`-o '--tag ~reboot'`), and whether
the images become a GitHub release.

* `matrix` turns the picked machines into the build matrix.
* `build` runs once per machine, one at a time, on the self-hosted runner:
  `image:name`, then `image:build`. It uploads the versioned
  `tessaro-os-<machine>-<version>-<sha>.wic.zst` and its `.wic.bmap` as the
  `image-<machine>` artifact. On qemux86-64 it then runs `e2e:setup` and
  `e2e:run` and uploads `build/e2e/` as `e2e-logs`.
* `release` runs only when asked and only when every build, e2e included,
  passed. It tags the built commit `v<version>-<sha>` and attaches every
  image and bmap.

**e2e runs in the build job, not after it.** The suite boots runqemu out of
the kas build tree that built the image (native qemu, OVMF, the
`qemuboot.conf`), so it needs that workspace, not a downloaded `.wic`.
`E2E_WORKER_OFFSET=10` moves its ports away from a workstation's own VMs and
`dev:tunnel`'s forwards on the same host. Its gems go under `build/`.

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
* **One build at a time.** The matrix has `max-parallel: 1`, and each job
  waits while `pgrep -af 'kas-container|bitbake'` finds anything, so a
  workstation build or e2e run on the host finishes first. A job cancelled
  mid-build can leave its kas container running; check `docker ps` before
  the next one.
* **Never trigger `image.yml` from `pull_request`.** A self-hosted runner
  runs whatever the workflow checks out; `workflow_dispatch` keeps that to
  people with write access to the repository.

### Setting it up

**`.github/setup-runner.sh` sets the runner up**, run on the build host as
the user who builds there, from a checkout. Running it again skips what is
done and refreshes the service's `PATH`.

```sh
.github/setup-runner.sh --repo OWNER/NAME --token TOKEN \
    [--work /big/disk/runner-work] [--cache /path/to/cache]
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
6. **Sets the `TESSARO_CACHE_DIR` variable** through `gh`, or prints what to
   set by hand. The default is `/srv/tessaro/cache` (`--cache`).

A new build host gets the shared cache first, as the user who builds there:

```sh
sudo mkdir -p /srv/tessaro && sudo chown "$USER": /srv/tessaro
mv cache /srv/tessaro/cache 2>/dev/null || mkdir -p /srv/tessaro/cache
ln -s /srv/tessaro/cache cache    # in the checkout
```
