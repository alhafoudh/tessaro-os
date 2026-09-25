# Updating a device

**In place, without A/B partitions and without signing, from the same
`.wic.bz2` and `.wic.bmap` that `image:flash` writes.** `tessaro-ctl --node
NAME update send IMAGE.wic.bz2` (or `mise run image:update NAME`) and the
device reboots into it; settings, the claim and the browser profile stay.
`--wipe-data` re-creates `/data` as well, and the device comes back unclaimed
with a new identity. Only the blocks the bmap lists are written, and only to
the root partition, plus the kernel file on the boot partition.
`--repartition` writes the whole disk instead - see **Rewriting the whole
disk** below.

1. **Upload.** 4 MiB base64 chunks over the control protocol (`update-begin`,
   `update-chunk`), each fsynced to `/data/tessaro/update/upload.part` before
   it is acknowledged. The same command run again resumes from the last byte
   the device has. The partition table is checked against the device's own
   after the first chunk, so a wrong image fails in seconds, not after the
   whole upload. `update-begin` refuses an upload `/data` has no room for:
   the upload, 128 MiB for the profile to keep writing, and for a root
   update the boot staging, measured as twice what the bmap maps inside
   the device's boot partition (`boot_room` in `updates.rs`) - a few tens of
   MB, since `boot.img` is sparse. It is measured, not the boot partition's
   size, because an ungrown `/data` is 1 GB: a fixed 512M left an update no
   room there. Only when the device's layout cannot be read is it 512M.
2. **Verify, then prepare**, in the agent, on a thread of its own at idle
   CPU and I/O priority while the kiosk keeps running. `verifying` checks the
   whole file against its SHA-256 (`update send --no-verify` skips it - the
   bmap's checksums below still cover every block that gets written);
   `preparing` is a dry run: one pass over the decompressed image, each bmap
   range up to the end of root checked against the bmap's own SHA-256, and
   the mapped parts of root re-cut into 4 MiB chunks with checksums of their
   own (`manifest.json`, with the upload's own SHA-256). Nothing of the image
   is kept but the kernel, copied out of a loop mount of the image's boot
   partition, staged sparse in `boot.img` for as long as that takes. The
   upload itself stays: it is what gets written. `/data` in the image is
   never decompressed.
3. **Commit** writes `pending` and reboots.
4. **Apply**, in the initramfs (`/init.d/80-tessaro_update`, before
   `90-rootfs`, so nothing has the root partition mounted): `tessaro-flash`
   checks the upload against the manifest's SHA-256 and the staged kernel
   *before the first write*, then decompresses the upload again, checks each
   chunk against its hash as it writes it, drops the device's page cache and
   reads everything back, then installs the kernel as `<name>.new` and
   renames it over the old one, and reboots into it. Decompressing is the
   slow part: seconds on x86, likely a few minutes on the Pi, with the screen
   showing the console's progress. The logic is
   `agent/update/src/{image,apply,flash,wipe}.rs`.
5. **Report**: the boot oneshot puts the result in `journalctl -t
   tessaro-config` once; `tessaro-ctl update status` shows it until the next
   update. `tessaro-ctl device status` shows `PRETTY_NAME`/`IMAGE_VERSION`.

Things to know:

* **A power cut is survivable at every step but one.** Before the first
  write the old system is intact. From the first write on, the marker and
  the upload are still on `/data`, so the next boot writes everything again
  - the half-written root is never mounted. `pending.started` is set just
  before the first write, so staging that stops verifying after that is not
  taken as a reason to boot the (gone) old root. Five attempts, then it stops
  on the console asking for a reflash. The one unprotected path is the ESP -
  a kernel that does not boot - and the bootloader is never touched.
* **The upload is checked whole before the initramfs writes a byte**, and
  that is what makes decompressing it twice safe. The dry run proved the
  file decompresses to what the bmap says; the SHA-256 check in the
  initramfs proves it is still that file. Without it, a file damaged on
  `/data` between the dry run and the apply would only show up at the chunk it hits, after
  root was half written, and every retry would hit it again.
* **Every identifier the rootfs names is pinned in the x86 wks files, and
  that is load-bearing.** wic makes new ones on every build otherwise: the
  PARTUUIDs (`--uuid`) go into grub.cfg's `root=`, and the `/boot` vfat
  serial (`--fsuuid`) into the rootfs's `/etc/fstab` as a `UUID=` line. The
  partition table and the ESP's filesystem are never
  rewritten, so an unpinned new rootfs would name another build's `/boot` and
  fail `local-fs.target`. The pins differ per machine, which makes the layout
  check a machine check too. qemux86-64 uses our own copy of Moonforge's wks
  for this.
* **The Pi cannot pin its disk signature** - this wic has no `--diskid`, and
  derives it from `SOURCE_DATE_EPOCH` - but nothing there names a PARTUUID
  (`root=/dev/mmcblk0p2`, device nodes in fstab), so for MBR images the
  layout check compares partition geometry instead.
* **The root partition is a fixed `TESSARO_ROOTFS_SIZE` (4096M), the ESP a
  fixed `TESSARO_ESP_SIZE` (256M), the Pi's boot partition 512M.** A later
  image has to fit the partition already on the disk, and the ESP holds two
  kernels during the swap. Each is sized well past the current rootfs and
  kernels on purpose, since growing one costs a reflash. wic fails the
  build if the rootfs outgrows it.
  Changing any partition is a new disk layout: every device needs one full
  reflash or one `--repartition`, which the updater says in so many words
  when it refuses.
* **The initramfs is bundled into the kernel** (`INITRAMFS_IMAGE_BUNDLE`), so
  the boot partition still has one kernel file to swap and bootimg-efi picks
  it up by itself - as `bzImage-initramfs-<machine>.bin`, which is the
  `KIOSK_KERNEL_FILE` the agent extracts. On the Pi the bundle is installed
  as `Image`, the name `boot.scr` loads. The price: any change to
  `tessaro-flash`, and so to the agent workspace, re-bundles the kernel, and
  every update then swaps it.
* **The initramfs is modelled on `core-image-initramfs-boot`, plus our
  update module** (`recipes-core/images/tessaro-initramfs.bb`, its own
  `inherit image` recipe): udev for `/dev/disk/by-*`,
  90-rootfs, finish. finish `switch_root`s to `/sbin/init`, which is still
  the overlayfs-etc preinit. It finds the ESP and `/data` as partitions 1 and
  3 of root's disk, which every Tessaro wks has. It first waits for root
  with 90-rootfs's own `rootdelay`/`roottimeout` loop: a USB or SD boot disk
  can appear after `/init` has started, and without the wait the hook finds
  no root, skips a pending update without a word and the old system boots.
  On an ordinary boot the cost
  is a read-only mount of the ESP and of `/data`, and an `ls`.
* **The `/etc` overlay keeps shadowing the image.** A file edited on the
  device stays edited across updates - an update replaces the lower layer
  only. `--wipe-data` is the way out.
* **A device not on the pinned layout cannot take updates** (random
  PARTUUIDs, a root partition sized to its build). One `image:flash` gets it
  onto the layout.

## Rewriting the whole disk

**`update send --repartition` (`mise run image:update --repartition NAME`) is
`image:flash` over the network**, for a device whose disk layout is not the
image's - the partition sizes changed, or the pins did. The whole image is
written from the partition table on: boot, root and an empty `/data`,
the bmap's mapped blocks only, like bmaptool. It implies `--wipe-data`, so the
device comes back unclaimed with a new identity, and `tessaro-ctl` forgets
it. The upload, the dry run (over the whole image this time, `/data`
included, and with no kernel to copy out) and the commit are the same as a
root update's; what differs is the initramfs (`apply_disk` in
`agent/update/src/flash.rs`).

* **The upload goes into RAM, because the disk update overwrites the `/data`
  it sits on.** tessaro-flash mounts a tmpfs of the upload's size plus 16 MiB on
  `/run/tessaro-update/ram`, copies it in and checks the copy's SHA-256 while
  `/data` is still there, so a refusal up to that point boots the old system
  like any other. The agent refuses at `update-begin` if MemTotal is short of
  the upload plus 192 MiB, and tessaro-flash again if MemAvailable is short
  of it plus 64 MiB. That is why the compressed file is what is kept: on the
  Pi's 1 GB the decompressed image would not fit.
* **A power cut from the first write on needs a physical reflash.** There is
  nothing left to write again from, and the partition table may already
  describe partitions that hold nothing yet. That is the price, and the
  confirmation says so; a write failing half way stops on the console (exit
  4), for the same reason.
* **Nothing may have the disk's filesystems mounted.** tessaro-flash unmounts
  the ESP and `/data` itself after the RAM copy is checked, ESP first so that
  a failure there can still be recorded on `/data`. Then it writes the whole
  disk device (`--disk`, `/dev/sda` or `/dev/mmcblk0`, from the hook).
* **The result lands on the new `/data`.** After the write, `BLKRRPART` makes
  the kernel read the new table (retried: udev in the initramfs can hold a
  partition open for a moment), the new partition 3 is mounted and
  `result.json` goes into it with `wiped_data`, so the boot oneshot reports
  `disk rewritten: NAME ..., /data re-created`. If that part fails, the disk
  is still complete and boots; only the report is lost.
* **The check is that it fits, not that it matches.** `layout::check_disk`
  wants partitions 1 to 3 in the image (the hook finds the ESP and `/data`
  by number), the same kind of table as the device's (GPT or MBR, which is
  also the only machine check left), and an image no larger than the disk.
  Writing the wrong machine's image is possible here and bricks the device
  until a reflash, like `image:flash` of the wrong file.
* **The backup GPT header ends up where the image ended, not at the end of
  the disk**, and `/data` stays at `IMAGE_DATA_MIN_SIZE` - both exactly as
  after `image:flash` with bmaptool, which does not relocate or grow anything
  either. The kernel logs a GPT warning about the backup header and boots.
  `tessaro-ctl storage grow` fixes both afterwards - see **Storage**. A
  device whose `/data` was grown gets the image's size back, empty.
* **The running image's initramfs does the work**, so a device whose image
  predates `--repartition` refuses it as a layout mismatch and needs one
  physical reflash.

## Storage

**`/data` is the last partition on every machine, and it gets the rest of
the disk only on command.** Every image carries it at `IMAGE_DATA_MIN_SIZE`,
and `image:flash` and `update send --repartition` leave it at that size
whatever the card or disk holds. `tessaro-ctl storage show` reports the
space past it as unallocated (in `WARN`, with the command to run), and
`tessaro-ctl storage grow` gives it to `/data` on the running device: no
reboot, `/data` stays mounted, the kiosk keeps running. `storage grow
--check` prints the plan and changes nothing; `--yes` skips the question.
`storage partitions` and `storage usage` list the partitions and every
mounted filesystem. The logic is `agent/tessaro-agent/src/storage.rs`; the
client is `agent/tessaro-ctl/src/storage.rs`.

* **The grow is online, and each step is safe to cut.** GPT only, `sfdisk
  --relocate gpt-bak-std` moves the backup header to the real end of the disk.
  `sfdisk -N 3` with `,+` moves the end of partition 3 there (clamped at 2 TiB
  on MBR). `partx -u -n 3` hands the kernel the new size through BLKPG, and
  `resize2fs` grows ext4 online. `BLKRRPART` (`fsutil::reread_partitions`,
  what the initramfs uses) is refused on a disk with a mounted partition, which
  is why the kernel is told through `partx` instead. The partition write is
  one table write, and the kernel journals an online resize, so a power cut
  or an agent restart in between leaves a consistent disk.
* **The plan is made from the disk every time**, from sysfs and the ext4
  superblock, never remembered. The partition step runs when the space after
  `/data` is at least `GROW_MIN` (64 MiB), and the filesystem step when the
  filesystem is that much smaller than its partition. So a grow cut short
  after the partition step is finished by running it again, and a second grow
  on a grown disk says there is nothing to do.
* **No layout has a swap partition**, so `/data` can be last. A device on an
  older layout with swap after `/data` is refused with the reason; one
  reflash or `--repartition` fixes it.
* **A grown `/data` changes nothing for updates.** The root update's layout
  check compares partitions 1 and 2 only, and `--wipe-data` re-creates the
  filesystem on the partition that is there, so it keeps the grown size.
* **It runs like the speed test**: on a `spawn_blocking` thread that streams
  a `StorageGrowEvent` per step, under a 10 minute deadline, one at a time.
  The start, each step and `storage: grew /data from A to B` go to the
  journal at info.
* **`sfdisk`, `partx` and `resize2fs` are `RDEPENDS` of `tessaro-kiosk`.**
  Nothing else in the image carried them.
* **The `storage.*` read-only keys never move a template.** `storage.size`,
  `.unallocated`, `.data_size`, `.data_free`, `.data_used` and `.root_free`
  work as placeholders, and the default debug template shows `/data`'s free
  space. `state::Live::moves` skips them: free space changes with every write
  to `/data`, and following it would re-render and restart onto each change.
  The debug screen re-renders every 5s anyway. `device status` has a `data`
  row with the same numbers.
