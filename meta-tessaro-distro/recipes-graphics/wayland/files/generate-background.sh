#!/bin/sh
# Render background.png from background.svg. Run by hand after editing the SVG
# and commit the PNG with it: the build uses the PNG as it is.
#
# The light noise dithers the dark gradients, which band visibly at 8 bits per
# channel without it. Needs rsvg-convert and ImageMagick.
set -eu
cd "$(dirname "$0")"
rsvg-convert background.svg |
    magick - -alpha off -attenuate 0.35 +noise Uniform -depth 8 PNG24:background.png
