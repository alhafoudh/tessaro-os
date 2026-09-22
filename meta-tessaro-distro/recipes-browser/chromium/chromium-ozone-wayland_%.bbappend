# The vendored bytemuck gates its two core::simd impls on a nightly *date*
# (rustversion::before/since 2026-01-27, see Lokathor/bytemuck#343). rustversion
# orders any stable compiler below every nightly date - upstream behaviour, the
# vendored crate is pristine dtolnay and chromium does not patch it - so the OE
# rustc selects the pre-2026-01-27 impl and dies on core::simd::LaneCount and
# SupportedLaneCount, neither of which rust 1.95 has. Upstream never sees this
# because chromium's own rustc is dev-channel. The patch keeps the new-API impl
# and drops the old one; it is scoped to bytemuck, which is the only non-test
# vendored crate using date bounds.
#
# %-suffixed so the bbappend survives a chromium version bump (the recipe
# file name carries the version). Note that touching this file or the patch
# re-runs do_patch and therefore the whole chromium build - batch any change
# here with the next version bump.
FILESEXTRAPATHS:prepend := "${THISDIR}/files:"
SRC_URI += "file://0001-bytemuck-drop-nightly-date-gated-simd-impls.patch"
