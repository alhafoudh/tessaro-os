# walnascar meta-clang's libcxx DEPENDS on virtual/compilerlibs, the generic
# provider name only newer oe-core defines (scarthgap spells it
# virtual/${TARGET_PREFIX}compilerlibs). libgcc is the compiler runtime newer
# core points that name at by default, so alias it here. Version wildcard on
# purpose: the bbappend must survive a gcc bump in oe-core.
PROVIDES:append = " virtual/compilerlibs"
