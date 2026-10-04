#!/bin/sh
# Package an already-built try-tessaro as "Try Tessaro.app" beside it:
# the launcher, the QEMU runtime (build-qemu-gpu.sh and bundle-qemu-macos.sh,
# remade when their pins change), tessaro-ctl, Tessaro.app and the image. stdout
# is the app path.
set -eu
[ "$(uname -s)" = Darwin ] || { echo "macOS is required" >&2; exit 1; }
usage="usage: package-macos.sh TRY_TESSARO TESSARO_CTL TESSARO_APP IMAGE.wic.zst"
binary=${1:?$usage}
ctl=${2:?$usage}
gui_app=${3:?$usage}
image=${4:?$usage}
for file in "$binary" "$ctl"; do
    [ -x "$file" ] || { echo "Missing executable: $file" >&2; exit 1; }
done
[ -d "$gui_app" ] || { echo "Missing app: $gui_app" >&2; exit 1; }
case "$image" in *.wic.zst) ;; *) echo "The image must be a .wic.zst: $image" >&2; exit 1 ;; esac
[ -f "$image" ] || { echo "Missing image: $image" >&2; exit 1; }

here=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
gui_dir=$(dirname "$here")
output_dir=$(CDPATH= cd -- "$(dirname "$binary")" && pwd)
version=$(sed -n 's/^version = "\([^"]*\)"$/\1/p' "$gui_dir/Cargo.toml")
[ -n "$version" ] || { echo "Missing GUI workspace version" >&2; exit 1; }

# QEMU is built once into build/qemu-gpu (build-qemu-gpu.sh does nothing
# while its pins hold), and the runtime made from it only when that build
# changed.
gpu="$(dirname "$gui_dir")/build/qemu-gpu"
sh "$here/build-qemu-gpu.sh" "$gpu"
runtime="$output_dir/qemu-runtime"
if [ "$(cat "$runtime/.qemu-version" 2>/dev/null)" != "$(cat "$gpu/.stamp")" ]; then
    sh "$here/bundle-qemu-macos.sh" "$runtime" "$gpu"
    cp "$gpu/.stamp" "$runtime/.qemu-version"
fi

app="$output_dir/Try Tessaro.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/bin" \
    "$app/Contents/Resources/image" "$app/Contents/Helpers"
cp "$binary" "$app/Contents/MacOS/try-tessaro"
cp "$ctl" "$app/Contents/Resources/bin/tessaro-ctl"
# macOS's own cp, whose -c clones on APFS: the image and the runtime cost no
# time and no space. mise puts GNU's first on the PATH.
/bin/cp -cR "$runtime" "$app/Contents/Resources/qemu"
rm -f "$app/Contents/Resources/qemu/.qemu-version"
/bin/cp -cR "$gui_app" "$app/Contents/Helpers/Tessaro.app"
/bin/cp -c "$image" "$app/Contents/Resources/image/"
cp "$here/icons/TryTessaro.icns" "$app/Contents/Resources/TryTessaro.icns"
cat >"$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key><string>en</string>
    <key>CFBundleExecutable</key><string>try-tessaro</string>
    <key>CFBundleIdentifier</key><string>sk.tessaro.try</string>
    <key>CFBundleName</key><string>Try Tessaro</string>
    <key>CFBundleDisplayName</key><string>Try Tessaro</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$version</string>
    <key>CFBundleVersion</key><string>$version</string>
    <key>CFBundleIconFile</key><string>TryTessaro.icns</string>
    <key>LSMinimumSystemVersion</key><string>13.0</string>
    <key>NSPrincipalClass</key><string>NSApplication</string>
    <key>NSHighResolutionCapable</key><true/>
    <key>NSMicrophoneUsageDescription</key>
    <string>The Tessaro device records from this microphone when Microphone is on in Try Tessaro's settings.</string>
</dict>
</plist>
EOF
# Not --deep: QEMU keeps the signature with its hypervisor entitlement.
codesign --force --sign - "$app/Contents/Resources/bin/tessaro-ctl" >&2
codesign --force --sign - "$app" >&2
printf '%s\n' "$app"
