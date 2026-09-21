# The OE rustc is a stable compiler; chromium's own rust toolchain is dev.
# The vendored rustversion fork only ever sees the dev channel upstream, and
# its stable-channel ordering breaks #[rustversion::before(date)] on stable:
# the pre-2026-01-27 simd block in bytemuck compiles and fails on
# core::simd::LaneCount, which no longer exists. See the patch.
#
# %-suffixed so the bbappend survives a chromium version bump (the recipe
# file name carries the version).
FILESEXTRAPATHS:prepend := "${THISDIR}/files:"
SRC_URI += "file://0001-rustversion-stable-newer-than-nightly-dates.patch"
