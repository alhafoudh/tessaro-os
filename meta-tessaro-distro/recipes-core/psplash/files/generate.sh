#!/bin/sh
# Render the splash PNGs from their SVGs. Run by hand after editing an SVG and
# commit the PNGs with it: the build uses the PNGs as they are.
#
# The alpha channel is dropped because psplash draws a pixel either fully or
# not at all, so an antialiased edge must already be blended into the
# background the SVG paints. Needs rsvg-convert and ImageMagick.
set -eu
cd "$(dirname "$0")"
for name in tessaro-splash psplash-bar; do
    rsvg-convert "$name.svg" | magick - -background '#0a0d14' -flatten -alpha off "PNG24:$name.png"
done
