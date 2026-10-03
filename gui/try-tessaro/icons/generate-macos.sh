#!/bin/sh
# try-tessaro.svg to the 1024px try-tessaro.png the window's header shows
# and the TryTessaro.icns the app bundle carries. Needs rsvg-convert
# (brew install librsvg); rerun after changing the SVG.
set -eu
cd "$(dirname "$0")"
rsvg-convert -w 1024 -h 1024 try-tessaro.svg -o try-tessaro.png
iconset=$(mktemp -d "${TMPDIR:-/tmp}/try-tessaro-icons.XXXXXX")
trap 'rm -rf "$iconset"' EXIT HUP INT TERM
mkdir "$iconset/TryTessaro.iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" try-tessaro.png \
        --out "$iconset/TryTessaro.iconset/icon_${size}x${size}.png" >/dev/null
    retina=$((size * 2))
    sips -z "$retina" "$retina" try-tessaro.png \
        --out "$iconset/TryTessaro.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset/TryTessaro.iconset" -o TryTessaro.icns
