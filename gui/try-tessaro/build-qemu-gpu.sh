#!/bin/sh
# Build the QEMU Try Tessaro bundles, with VirGL on macOS's own OpenGL, into
# OUT_DIR (docs/try-tessaro.md, "The QEMU runtime"):
#
#   install/       qemu-system-aarch64, qemu-img and the edk2 firmware
#   deps/          virglrenderer, built here; libepoxy and ANGLE, as bottles
#   licenses/      each of those four's license texts
#   components.json  the four, in `brew info --json=v2`'s shape, for the SBOM
#
# It is Try Omarchy's GPU recipe (github.com/omacom/try-omarchy,
# macos/build-qemu-gpu-runtime.sh) and nothing else of theirs: QEMU 11.1.1
# with their Cocoa OpenGL, fence polling and refresh patches, virglrenderer
# 1.3.0 with the startergo/homebrew-virglrenderer patch set and their native
# OpenGL patch, and startergo's libepoxy and ANGLE bottles. Every download is
# pinned by sha256; the patches are fetched at a pinned commit rather than
# copied into this repo. glib, pixman, libslirp and dtc are Homebrew's, as
# the bundle always took them.
#
# Slow (QEMU is a full build), so it does nothing when OUT_DIR/.stamp holds
# this script's own hash: every pin is in this file.
set -eu
[ "$(uname -s)" = Darwin ] || { echo "macOS is required" >&2; exit 1; }
out=${1:?usage: build-qemu-gpu.sh OUT_DIR}
here=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
mkdir -p "$out"
out=$(CDPATH= cd -- "$out" && pwd)

stamp=$(shasum -a 256 "$here/build-qemu-gpu.sh" | cut -c1-64)
if [ "$(cat "$out/.stamp" 2>/dev/null)" = "$stamp" ] && [ -x "$out/install/bin/qemu-system-aarch64" ]; then
    exit 0
fi

qemu_version=11.1.1
qemu_sha256=079ffbff8a7111bbc89022107cbabf3bbfd614d5fc9d7cc675991196aca12482
virgl_version=1.3.0
virgl_sha256=065bc56e89e6f631f96101cd62eba0748e48eb888b434edc86e89d05395e76f3
virgl_tap_version=1.0.42
virgl_tap_sha256=950273fbba46905b6112ee2bd0598c1da706c25319a7347058cbc52f04ba96dd
epoxy_bottle=1.0.5
epoxy_version=1.5.11
epoxy_sha256=109384a1d37edf207a9b9f3d8950710c00767635b3c7ff295e3af83611876ef2
angle_bottle=1.0.16
angle_sha256=29fe2175b157a65f12879f9a12b5c8f94d0a76fafdf41ff009a2fdb4e9df525c
omarchy_commit=82927e98078a452ace33e527a328ef0b12a0af07
# Try Omarchy's patches, in the order they apply, with their sha256.
qemu_patches="
qemu-texture-borrowing-11.1 b20bdf9a7d7ccda5b86366ad9d09a3bf95308b98a06b1ece281344405bcc7ab9
qemu-gpu-spike-resolution-fix b554e1ef9910d0891d69ee0fe84e479559c057dc28291e36e1524031808fc69f
qemu-darwin-gpu-fence-poll 1ac407bdb617dfc52d004d0ebd0d07641d920f7d3a9756223c6426a207fb1499
"
virgl_native_patch_sha256=692ed73cf88780b4c0e04c56e3cfb21cec761768dea909d755624e07d82fc60c
# The tap's own list, as its 1.0.42 formula and Try Omarchy apply it.
virgl_tap_patches="
virglrenderer-debug-init-logging
virglrenderer-default-debug-log
virglrenderer-macos-unified
virglrenderer-venus-metal-func-ptrs
virglrenderer-gallium-endian
virglrenderer-macos-a8-swizzle
virglrenderer-corefoundation-link
virglrenderer-a8-shader-swizzle
virglrenderer-a8-shader-swizzle-texture
virglrenderer-a8-unpack-alignment
virglrenderer-bgra-upload-swizzle-core
virglrenderer-msaa-assertion-fix
virglrenderer-ignore-surface0-clear
virglrenderer-venus-errno-debug
virglrenderer-macos-profile-forcing
virglrenderer-macos-egl-profile
virglrenderer-texture-swizzle-core
virglrenderer-bgra-unified
virglrenderer-core-profile-frag-datalocation
virglrenderer-macos-core-profile-fixes
virglrenderer-gles-dual-source-output
"

for brew_formula in glib pixman libslirp dtc pkgconf; do
    brew --prefix --installed "$brew_formula" >/dev/null 2>&1 || {
        echo "needs Homebrew's $brew_formula: brew install glib pixman libslirp dtc pkgconf" >&2
        exit 1
    }
done

downloads="$out/downloads"
src="$out/src"
deps="$out/deps"
rm -rf "$src" "$deps" "$out/install" "$out/licenses" "$out/.stamp"
mkdir -p "$downloads" "$src" "$deps" "$out/licenses"

# fetch URL FILE SHA256: into downloads/, kept for the next build.
fetch() {
    file="$downloads/$2"
    if [ ! -f "$file" ] || [ "$(shasum -a 256 "$file" | cut -c1-64)" != "$3" ]; then
        echo "Downloading $2" >&2
        curl --fail --location --silent --show-error --retry 3 --proto '=https' -o "$file.part" "$1"
        mv "$file.part" "$file"
    fi
    actual=$(shasum -a 256 "$file" | cut -c1-64)
    [ "$actual" = "$3" ] || { echo "$2: sha256 $actual, expected $3" >&2; exit 1; }
}

fetch "https://download.qemu.org/qemu-$qemu_version.tar.xz" "qemu-$qemu_version.tar.xz" "$qemu_sha256"
fetch "https://gitlab.freedesktop.org/virgl/virglrenderer/-/archive/$virgl_version/virglrenderer-$virgl_version.tar.gz" \
    "virglrenderer-$virgl_version.tar.gz" "$virgl_sha256"
fetch "https://codeload.github.com/startergo/homebrew-virglrenderer/tar.gz/refs/tags/v$virgl_tap_version" \
    "homebrew-virglrenderer-$virgl_tap_version.tar.gz" "$virgl_tap_sha256"
fetch "https://github.com/startergo/homebrew-libepoxy/releases/download/v$epoxy_bottle/libepoxy-$epoxy_bottle.arm64_sequoia.bottle.tar.gz" \
    "libepoxy-$epoxy_bottle.arm64_sequoia.bottle.tar.gz" "$epoxy_sha256"
fetch "https://github.com/startergo/homebrew-angle/releases/download/v$angle_bottle/angle-$angle_bottle.arm64_sequoia.bottle.tar.gz" \
    "angle-$angle_bottle.arm64_sequoia.bottle.tar.gz" "$angle_sha256"
omarchy="https://raw.githubusercontent.com/omacom/try-omarchy/$omarchy_commit/macos/patches"
for name in $(echo "$qemu_patches" | awk '{ print $1 }'); do
    fetch "$omarchy/$name.patch" "$name.patch" "$(echo "$qemu_patches" | awk -v n="$name" '$1 == n { print $2 }')"
done
fetch "$omarchy/virgl-native-opengl.patch" virgl-native-opengl.patch "$virgl_native_patch_sha256"

# Meson, Ninja and PyYAML (virglrenderer's generated tables), pinned, in a
# venv of their own.
venv="$out/venv"
[ -x "$venv/bin/meson" ] || {
    python3 -m venv "$venv"
    "$venv/bin/pip" install --quiet meson==1.9.0 ninja==1.13.0 PyYAML==6.0.3
}
PATH="$venv/bin:$PATH"
jobs=$(sysctl -n hw.ncpu)

# The bottles name themselves by Homebrew's paths. Each dylib takes its
# path here as its id, which is what QEMU and virglrenderer then link to and
# bundle-qemu-macos.sh follows, and the .pc files point here.
tar -xzf "$downloads/libepoxy-$epoxy_bottle.arm64_sequoia.bottle.tar.gz" -C "$src"
tar -xzf "$downloads/angle-$angle_bottle.arm64_sequoia.bottle.tar.gz" -C "$src"
mv "$src/libepoxy/$epoxy_bottle" "$deps/libepoxy"
mv "$src/angle/$angle_bottle" "$deps/angle"
chmod -R u+w "$deps"
for lib in "$deps/libepoxy/lib/libepoxy.0.dylib" "$deps/angle/lib/libEGL.dylib" "$deps/angle/lib/libGLESv2.dylib"; do
    install_name_tool -id "$lib" "$lib" 2>/dev/null
    codesign --force --sign - "$lib" 2>/dev/null
done
ln -sf libepoxy.0.dylib "$deps/libepoxy/lib/libepoxy.dylib"
for pc in "$deps"/libepoxy/lib/pkgconfig/*.pc "$deps"/angle/lib/pkgconfig/*.pc; do
    awk -v prefix="$(dirname "$(dirname "$(dirname "$pc")")")" \
        '/^prefix=/ { print "prefix=" prefix; next } { print }' "$pc" >"$pc.new"
    mv "$pc.new" "$pc"
done
pkg_config_path="$deps/virglrenderer/lib/pkgconfig:$deps/libepoxy/lib/pkgconfig:$deps/angle/lib/pkgconfig"

echo "Building virglrenderer $virgl_version" >&2
tar -xzf "$downloads/virglrenderer-$virgl_version.tar.gz" -C "$src"
tar -xzf "$downloads/homebrew-virglrenderer-$virgl_tap_version.tar.gz" -C "$src"
virgl="$src/virglrenderer-$virgl_version"
for name in $virgl_tap_patches; do
    patch -d "$virgl" -p1 -f -s -i "$src/homebrew-virglrenderer-$virgl_tap_version/patches/$name.patch"
done
patch -d "$virgl" -p1 -f -s -i "$downloads/virgl-native-opengl.patch"
PKG_CONFIG_PATH="$pkg_config_path" CFLAGS="-I$deps/angle/include" \
    LDFLAGS="-Wl,-headerpad_max_install_names" \
    meson setup "$virgl/build" "$virgl" --prefix="$deps/virglrenderer" --libdir=lib \
    --buildtype=debugoptimized -Db_ndebug=false --wrap-mode=nodownload \
    -Ddrm-renderers=[] -Dvenus=true -Dtests=false -Dvideo=false -Dtracing=none >&2
ninja -j "$jobs" -C "$virgl/build" >&2
meson install -C "$virgl/build" --no-rebuild >&2

echo "Building QEMU $qemu_version" >&2
tar -xJf "$downloads/qemu-$qemu_version.tar.xz" -C "$src"
qemu="$src/qemu-$qemu_version"
for name in $(echo "$qemu_patches" | awk '{ print $1 }'); do
    patch -d "$qemu" -p1 -f -s -i "$downloads/$name.patch"
done
# configure's own venv installs offline (--disable-download) from
# python/wheels, which lacks the packaging tools it installs qemu.qmp with.
cat >"$src/wheels.txt" <<'EOF'
setuptools==84.0.0 --hash=sha256:51a52592b3b99e102b609654876bd65f19f999935166d1352678931132b0c670
wheel==0.48.0 --hash=sha256:3217dcc807155e45db462d7ef2431f5ddda0d7273b700d05a67b271ceb1287ab
packaging==26.3 --hash=sha256:d7193f7c8e4e93f444fde0262bf90af30e16fa0ad0ad44cb553c87339b23cd1c
pip==26.2.1 --hash=sha256:71138adf1f4ca900cdb7d289c21b7494329f2332b6d85f0e1c42108c0384ed3e
EOF
pip download --quiet --no-deps --only-binary=:all: --require-hashes -r "$src/wheels.txt" \
    -d "$qemu/python/wheels" >&2
mkdir "$qemu/build"
(
    cd "$qemu/build"
    PKG_CONFIG_PATH="$pkg_config_path" ../configure \
        --prefix="$out/install" \
        --target-list=aarch64-softmmu \
        --without-default-features \
        --enable-system --enable-tools \
        --enable-hvf --disable-tcg \
        --enable-cocoa --enable-opengl --enable-virglrenderer \
        --enable-pixman --enable-slirp --enable-fdt=system \
        --enable-coreaudio --audio-drv-list=coreaudio \
        --disable-docs --disable-debug-info --disable-werror --disable-download \
        --extra-cflags="-I$(brew --prefix)/include" \
        --extra-ldflags="-L$(brew --prefix)/lib -Wl,-headerpad_max_install_names" >&2
)
ninja -j "$jobs" -C "$qemu/build" >&2
# Installing signs QEMU with the hypervisor entitlement (QEMU's own
# scripts/entitlement.sh), so it also runs in place (scripts/qemu-arm64.sh).
ninja -C "$qemu/build" install >&2

# The license texts of what this built or fetched, and the SBOM's view of
# them. ANGLE's bottle carries no license file: name it and its source.
mkdir -p "$out/licenses/qemu" "$out/licenses/virglrenderer" "$out/licenses/libepoxy" "$out/licenses/angle"
cp "$qemu/COPYING" "$qemu/COPYING.LIB" "$qemu/LICENSE" "$out/licenses/qemu/"
cp "$out/install/share/qemu/edk2-licenses.txt" "$out/licenses/qemu/"
cp "$virgl/COPYING" "$out/licenses/virglrenderer/"
cp "$deps/libepoxy/COPYING" "$out/licenses/libepoxy/"
cat >"$out/licenses/angle/NOTICE" <<EOF
ANGLE, BSD-3-Clause: https://chromium.googlesource.com/angle/angle/+/main/LICENSE
Built by https://github.com/startergo/homebrew-angle, release v$angle_bottle.
EOF
cat >"$out/components.json" <<EOF
{"formulae": [
  {"name": "qemu", "license": "GPL-2.0-only", "homepage": "https://www.qemu.org/",
   "versions": {"stable": "$qemu_version"}},
  {"name": "virglrenderer", "license": "MIT", "homepage": "https://gitlab.freedesktop.org/virgl/virglrenderer",
   "versions": {"stable": "$virgl_version"}},
  {"name": "libepoxy", "license": "MIT", "homepage": "https://github.com/anholt/libepoxy",
   "versions": {"stable": "$epoxy_version"}},
  {"name": "angle", "license": "BSD-3-Clause", "homepage": "https://chromium.googlesource.com/angle/angle",
   "versions": {"stable": "startergo $angle_bottle"}}
]}
EOF

# The sources are large and only the install is used.
rm -rf "$src"
printf '%s\n' "$stamp" >"$out/.stamp"
"$out/install/bin/qemu-system-aarch64" --version | head -n 1 >&2
