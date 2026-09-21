# meta-clang is pinned on its walnascar branch for clang 20 (see
# kas/repo/meta-chromium.yml), whose clang-cross DEPENDS on
# virtual/cross-binutils - a provider name only newer oe-core defines. Provide
# it here: this is the same cross linker the virtual/${TARGET_PREFIX}binutils
# name below resolves to. A %-suffixed bbappend because the recipe's PN is
# arch-specific (binutils-cross-x86_64), so a :pn- override cannot target it.
PROVIDES:append = " virtual/cross-binutils"
