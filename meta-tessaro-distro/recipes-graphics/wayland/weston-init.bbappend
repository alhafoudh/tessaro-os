# Replace Moonforge's wallpaper with a diagnostic one: light blue with a yellow
# circle. Moonforge ships a plain white background.png, which is impossible to
# tell apart from an uninitialised framebuffer or a blank browser window - the
# exact ambiguity that made the kiosk bring-up hard to read under QEMU.
#
# meta-moonforge-graphics already puts "file://background.png" in SRC_URI and
# installs it. This layer has the higher BBFILE_PRIORITY (20 vs 6), so its
# bbappend is parsed later and this prepend lands ahead of theirs in FILESPATH,
# which is what makes our copy the one that gets picked up.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

# NOTE: do not add "use-pixman" here. It was tried while chasing the blank
# kiosk under QEMU, and it makes things worse rather than better: the pixman
# renderer stops Weston advertising linux-dmabuf, and WPE 2.52 has no wl_shm
# fallback, so the browser fails with "no valid format found" instead. The
# real answer is to give QEMU a working GPU (virtio-vga-gl + virgl), which
# needs /dev/dri on the host - see the "QEMU needs a real GPU" note in
# CLAUDE.md.
