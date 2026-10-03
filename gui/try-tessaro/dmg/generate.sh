#!/bin/sh
# background.svg to background.tiff, the DMG window's background at 1x and
# 2x in one file, so Finder picks the sharp one on a Retina screen. Needs
# rsvg-convert (brew install librsvg); rerun after changing the SVG.
set -eu
cd "$(dirname "$0")"
work=$(mktemp -d "${TMPDIR:-/tmp}/try-tessaro-dmg.XXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM
rsvg-convert -w 660 -h 420 background.svg -o "$work/background.png"
rsvg-convert -w 1320 -h 840 background.svg -o "$work/background@2x.png"
tiffutil -cathidpicheck "$work/background.png" "$work/background@2x.png" -out background.tiff
