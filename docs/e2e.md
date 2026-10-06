# The agent's end-to-end checks

`mise run e2e:run` boots the qemux86-64 image and runs the RSpec suite in
`test/e2e/spec/` against it. Each case provokes one thing the agent exists to
handle - a site going down, a crashed or wedged browser, a config change, a
claim, a network change that has to roll back, an image update - and asserts
on the lines it writes to its journal. The spec files are the list. Exits 1
on a failing case, printing the journal lines it saw under the failure, and 2
when it cannot start at all (no image).

Running it: `mise run e2e:setup` once (rspec and parallel_tests, in
`test/e2e/Gemfile`), then `mise run e2e:run`. `E2E_JOBS=N` is how many VMs
run at once (3 by default, 1 for one at a time); `mise run e2e:one --
spec/network_spec.rb -e ping` runs one lane or case with plain rspec, with
paths relative to `test/e2e`, and `mise run e2e:one -- --only-failures`
reruns what failed last time (from `build/e2e/rspec-status.txt`).
`E2E_VERBOSE=1` prints each step of a case as it starts - guest commands,
journal waits and what matched, CDP calls, deliberate sleeps - and `2` adds
every agent journal line a wait sees. Every lane's steps go to
`build/e2e/<lane>.log` whatever the verbosity, and its console to
`build/e2e/<lane>.qemu.log`. `E2E_KEEP=1` leaves a VM up after its lane,
`E2E_REUSE=1` runs against one already up on worker 0's ports, and
`-o '--tag ~reboot'` leaves out the update lanes. Output is live, each line
prefixed with its worker, and after every case one `== progress ...` line
covers the whole run: the workers meet in `build/e2e/progress/`
(`spec/support/progress.rb`), which `mise run e2e:run` empties before they
start.

* **The suite never builds the image.** It refuses to start without a `.wic`
  and warns when the image is older than anything under `agent/` or
  `meta-tessaro-distro/`, since an old agent passes and proves nothing. It is
  a warning so an older image can still be tested on purpose.
* **A spec file is a lane, and a lane is a VM.** The cases of a file run in
  order (`config.order = :defined`) and leave state behind for each other,
  so the shared context in `spec/support/booted_vm.rb` boots a fresh VM
  before a file's first case and powers it off after its last. That is per
  file, not per worker: parallel_tests runs several files one after the other
  on a worker, and a lane must never inherit another's VM. A new case goes
  into the lane whose state it fits; a case that reboots goes into a file of
  its own, tagged `:reboot`. A lane gets a disk larger than the image with
  `extra_disk:` on its describe (`spec/support/vm.rb`), which boots a grown
  sparse copy and leaves the image as built.
* **Every worker has its own ports**, from `TEST_ENV_NUMBER`
  (`spec/support/ports.rb`); worker 0 has the ports a single VM always had.
  `E2E_WORKER_OFFSET=1` moves every worker one up, for a host where
  something else holds worker 0's ports (`dev:tunnel` holds 7400 on the
  build host).
  The forwards are fixed in the image's qemuboot.conf, so each worker writes
  its own copy, `tessaro-os-qemux86-64.e2e-worker-N.qemuboot.conf`, into the
  deploy directory and passes it to runqemu *after* the `.wic`: runqemu
  derives a conf from the image argument, only a later conf argument replaces
  it, and it takes the conf's own directory as `DEPLOY_DIR_IMAGE`, where it
  looks for the kernel and OVMF. runqemu moves a forward whose port is taken
  and logs `Port forward changed`; the harness fails on that line rather than
  drive another worker's VM.
* **Cleanup is hooks and `ensure`.** `:reconfigure` on a case puts the test
  settings back and waits for a settled agent after it, whatever happened;
  cleanup particular to one case stays in its `ensure`, which runs first.
* **Steps come from the helpers, not from the cases.** `Guest#run`,
  `Journal#wait_for`/`refute` and `Cdp#command` write one line each via
  `AgentE2E.step` (`spec/support/output.rb`). Plumbing (cursors, journal
  reads, unit properties) and every polling loop run inside `quietly`, with
  one `step` naming the wait before it, or the log would get a line per poll.
  A sleep that is part of what a case proves is `pause SECONDS, "why"`.
* **The VM has sound cards `mise run qemu:run` does not.** `support/vm.rb` passes
  runqemu `qemuparams=` for an Intel HDA line out and a USB audio device,
  each on a `-audiodev wav` recording to
  `build/qemux86-64/e2e-worker-N.{jack,usb}.wav`, so the audio lane reads
  which card a sound came out of from the host. They are not in the kas
  fragment, or every `mise run qemu:run` would write WAV files. QEMU's wav backend
  cannot capture, so the mic case only proves the grant.
* **The camera lane plugs in a camera from the host, over USB/IP.** qemu
  emulates no camera, so `camera_spec.rb` starts `test/usbcam/usbcam.rb`
  on the worker's USB/IP port (3240 plus the worker's offset,
  `support/usbcam.rb`) and the guest attaches it with `usbip attach -r
  10.0.2.2`: no forward is needed, the guest connects out through slirp.
  The emulator's log is `build/e2e/camera.usbcam.log`. How it works is
  **Testing in qemu** in [camera.md](camera.md).
* **The printer lane prints to CUPS's own test printer.** qemu emulates no
  printer, so `printer_spec.rb` runs `ippeveprinter` (from the image's
  `cups` package) in the guest on a loopback port, keeping every job as a
  file: a job reached the printer when a file appears. A printer that does
  not answer is a URI with nothing listening, where a job waits.
* **The playlist lane serves a site from the host.** A page that forbids
  framing and an image for the media cache have to come from somewhere that
  is not the device, so `playlist_spec.rb` runs a small server
  (`support/host_web.rb`) on the worker's `web` port (18080 plus the
  worker's offset) on the host's loopback, which the guest reaches as
  `http://10.0.2.2:<port>/` through slirp, like the camera lane's USB/IP
  server. The server keeps every path it was asked for: the framed page
  asks for `/e2e-rendered` from its script, which only a document the
  browser rendered does, so that request proves the frame unlock extension
  let it in.
* **It boots its own VM, not through `mise run qemu:run`.** The guest is driven over
  SSH, and runqemu's slirp forwards the loopback inside the kas container's
  network namespace, where `-p` publishing cannot reach it, so the harness
  passes `--network=host`. The unclaimed device's empty root password is the
  credential, which is why the suite never leaves the device claimed: the
  claim cases claim, check and unclaim inside one SSH command, with a
  local-socket unclaim in a `trap`; `ssh-key`, which has to log in by key
  while claimed, first starts a guard on the guest that unclaims after a
  timeout if no key gets in. The `webconfig` lane claims over the API
  instead, and between its claim and unclaim goes nowhere near SSH: it waits
  on the API, and every case unclaims with its token in `ensure`.
* **A browser is `Browser` in `support/api.rb`**: the API from the host
  with `Origin` and `Sec-Fetch-Site` as a browser sends them, keeping the
  cookies it is given, so session cases need no real browser.
* **It retunes the agent for the run** with `tessaro-ctl config set --no-apply` over
  the guest's local socket (short probes and backoff, no periodic refresh) and
  unsets those keys afterwards. The VM runs with `snapshot`, so a power-off
  discards everything anyway. mDNS cannot be exercised: slirp carries no
  multicast.
* **DevTools is driven from the host** through an SSH tunnel to the guest's
  `127.0.0.1:9222`, with a small websocket client in the script - the image has
  no curl or Python. Chromium's DevTools HTTP server answers HTTP/1.0 with
  nothing at all and holds 1.1 connections open, so the client reads by
  `Content-Length`.
* **VNC is driven from the host the same way**, through a tunnel to the
  guest's `127.0.0.1:5900` on the worker's `vnc_tunnel` port (15900 plus the
  worker's offset), opened only for the case that needs it
  (`spec/support/vnc.rb`). Its RFB client logs in the way TigerVNC does
  (VeNCrypt) or the way macOS Screen Sharing does (the classic VNC password,
  over RFB 3.8 or 3.3), and sends clicks and keys only; what reached the page
  is read over DevTools. The classic password's DES is triple DES with the
  key three times, which is single DES and needs no OpenSSL legacy provider.
* **The guest's BusyBox has `pgrep` but no `pkill`**, and `pgrep -f` also
  matches the remote shell running the kill. The harness brackets one
  character (`--type=rendere[r]`); unbracketed, `SIGKILL` ends its own SSH
  session and `SIGSTOP` freezes it.
