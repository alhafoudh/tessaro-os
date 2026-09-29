# Hardware identity and memory

**`tessaro-ctl device status` and the GUI's Overview say what the device
is - vendor, model, board, firmware, CPU, serial - and how much RAM it has
and uses, read from what the kernel already exposes.** Nothing extra runs
on the device for it: `agent/tessaro-agent/src/hardware.rs` reads a few
files inside `status` (`Control::status` in `control/mod.rs`) and fills
`Status.hardware` and `Status.memory` (`protocol::Hardware`,
`protocol::MemUsage`). Both are `#[serde(default)]`, so a client and an
agent of different ages still read each other; an agent without them just
shows no hardware rows.

* **DMI on x86, the device tree on the Pi; DMI wins when both exist.**
  From `/sys/class/dmi/id`: `sys_vendor`, `product_name` with
  `product_version` in parentheses (Lenovo puts the marketing name there),
  `board_vendor`/`board_name` (the vendor is left out when it is the
  system's), `bios_version` and `bios_date` as the firmware, and
  `product_serial`, else `board_serial`. The serial files are readable only
  by root, which the agent is. From `/proc/device-tree`: `model`,
  `serial-number`, the vendor from the first `compatible` entry
  (`raspberrypi` reads as "Raspberry Pi") and the SoC from the last
  (`brcm,bcm2837` reads as `BCM2837`). The board on a Pi is who built it,
  decoded from `system/linux,revision` (bits 16-19 of a new-style revision
  code, bit 23 set; the Raspberry Pi documentation's "Raspberry Pi revision
  codes"). Device-tree values are NUL-terminated; the NULs are trimmed.
* **Placeholders firmware vendors leave in DMI count as absent**
  ("To Be Filled By O.E.M.", "Default string", "System Product Name", an
  all-zero serial and the like, `PLACEHOLDERS` in `hardware.rs`), so a
  cheap board shows fewer rows rather than nonsense ones. A field that is
  absent is left out of both clients, never printed empty.
* **The CPU is `/proc/cpuinfo`'s `model name`, else the SoC.** arm64's
  cpuinfo has no `model name`. The core count is the number of `processor`
  entries, and the architecture is the agent's own build target
  (`std::env::consts::ARCH`).
* **Memory is `/proc/meminfo`'s `MemTotal` and `MemAvailable`**, used being
  their difference: page cache the kernel can drop counts as free, as in
  `free`'s "available" column. `meminfo_bytes` is shared with the update's
  `fits_in_ram` check (docs/updates.md).
* **CPU use is sampled by the agent, every 2s, for every caller.**
  `watch_cpu` (`control/watchers.rs`) reads the aggregate `cpu` line of
  `/proc/stat` and keeps the busy share of the ticks since the sample
  before (idle and iowait count as not busy; 100% is every core busy).
  A share needs two samples, and taking them between one client's calls
  would give a lone `device status` nothing, or a share over hours, and let
  two polling clients shorten each other's interval. `Status.cpu_percent`
  is `None` for the first half second after the agent starts: the second
  sample comes early (`FIRST`), because a `config set` often restarts the
  agent and a `status` right after it should still have a share. The GUI's status bar
  shows it next to RAM use, both refreshed on its 2s status poll
  (`POLL` in `gui/tessaro-gui/src/worker.rs`).
* **Everything else is read on each `status` call, not cached at start.** The
  reads are a handful of small sysfs and procfs files, memory has to be
  current anyway, and there is no startup state to get wrong. They run under
  `deadline::blocking`, like the os-release read next to them.
* **The serial is in `Status` and the page bridge, and nowhere else.**
  `Status` goes only to a caller allowed to manage the device, who could
  read the same files over SSH, and the kiosk's own page gets it through
  `tessaro.device.status()` (`page_status` in `control/bridge.rs`,
  docs/bridge.md), so a site can tell which unit it runs on. It stays out
  of `NodeInfo`, mDNS and the clients' known nodes, which the node id is built to keep
  free of anything that identifies the machine itself (the top of
  `identity.rs`).
* **The test fixtures never read this host's hardware.** `KIOSK_DMI`,
  `KIOSK_DEVICE_TREE`, `KIOSK_CPUINFO` and `KIOSK_MEMINFO` point the paths
  elsewhere (`paths.rs`); the control fixture in `control/mod.rs` sets them
  to a made-up qemu VM.
