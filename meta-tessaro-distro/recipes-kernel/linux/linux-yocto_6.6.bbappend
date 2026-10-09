# genericx86-64 builds the same linux-yocto 6.6 as genericarm64: oe-core's
# own SRCREV_machine and LINUX_VERSION in linux-yocto_6.6.bb. meta-yocto-bsp
# pins the generic x86 machines to an older revision
# (meta-yocto-bsp/recipes-kernel/linux/linux-yocto_6.6.bbappend), months of
# stable fixes behind, and with a hard `=` on LINUX_VERSION. This layer's
# priority is above meta-yocto-bsp's, so its bbappend is parsed later and
# these win; `bitbake -e linux-yocto | grep -E '^(LINUX_VERSION|SRCREV_machine)='`
# shows which applies.
#
# The values are linux-yocto_6.6.bb's defaults; copy them again after an
# oe-core bump. A mismatch does not go unnoticed: kernel-yocto's
# do_kernel_version_sanity_check fails the build when LINUX_VERSION and the
# checked-out source disagree.
#
# Named for 6.6 rather than %-suffixed like linux-yocto_%.bbappend: a
# revision only means something for the recipe it came from.
SRCREV_machine:genericx86-64 = "a8a7d078f151a24e01d4501853c88c6b08c9cad9"
LINUX_VERSION:genericx86-64 = "6.6.142"
