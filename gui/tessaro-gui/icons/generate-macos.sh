#!/bin/sh
# Export tessaro.svg as a transparent 1024px tessaro.png before running this.
set -eu
cd "$(dirname "$0")"
iconset=$(mktemp -d "${TMPDIR:-/tmp}/tessaro-icons.XXXXXX")
trap 'rm -rf "$iconset"' EXIT HUP INT TERM
mkdir "$iconset/Tessaro.iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" tessaro.png \
        --out "$iconset/Tessaro.iconset/icon_${size}x${size}.png" >/dev/null
    retina=$((size * 2))
    sips -z "$retina" "$retina" tessaro.png \
        --out "$iconset/Tessaro.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset/Tessaro.iconset" -o Tessaro.icns
