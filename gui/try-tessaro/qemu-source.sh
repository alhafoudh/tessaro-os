#!/bin/sh
# Pack the source of the QEMU Try Tessaro bundles into OUT.tar, from what
# build-qemu-gpu.sh downloaded into GPU_DIR: the QEMU and virglrenderer
# release tarballs, the virglrenderer patch set, Try Omarchy's patches, and
# build-qemu-gpu.sh itself, which says how they go together. QEMU is GPL and
# carries patches, so a release attaches this beside the DMG
# (docs/try-tessaro.md, "The QEMU runtime"). stdout is the archive's path.
set -eu
usage="usage: qemu-source.sh OUT.tar GPU_DIR"
archive=${1:?$usage}
gpu=${2:?$usage}
here=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
downloads="$gpu/downloads"
[ -d "$downloads" ] || { echo "no downloads in $gpu: run build-qemu-gpu.sh" >&2; exit 1; }

name=$(basename "$archive" .tar)
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/$name"
# Sources only: the libepoxy and ANGLE bottles are binaries, MIT and BSD.
for file in "$downloads"/qemu-*.tar.xz "$downloads"/virglrenderer-*.tar.gz \
    "$downloads"/homebrew-virglrenderer-*.tar.gz "$downloads"/*.patch; do
    [ -f "$file" ] || { echo "missing in $downloads: $(basename "$file")" >&2; exit 1; }
    cp "$file" "$stage/$name/"
done
cp "$here/build-qemu-gpu.sh" "$stage/$name/"
cat >"$stage/$name/README" <<'EOF'
The source of the QEMU that Try Tessaro bundles.

build-qemu-gpu.sh is the build: it names every file here with its sha256,
where it came from, which patches apply to which tree and in what order, and
how QEMU and virglrenderer are configured. Run it on macOS with these files
in OUT_DIR/downloads/ to build it again; it fetches the libepoxy and ANGLE
binaries it links, which are not here.
EOF
mkdir -p "$(dirname "$archive")"
tar -cf "$archive" -C "$stage" "$name"
printf '%s\n' "$archive"
