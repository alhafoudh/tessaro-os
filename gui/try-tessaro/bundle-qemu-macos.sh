#!/bin/sh
# Copy Homebrew's QEMU into OUT_DIR as a self-contained runtime for Try
# Tessaro: bin/ with qemu-system-aarch64 and qemu-img, lib/ with every
# non-system dylib they load (relinked to @executable_path/../lib),
# share/qemu/ with the UEFI firmware only, LICENSES/ per Homebrew formula,
# and runtime.json, `brew info --json=v2` of every formula shipped, which
# the SBOM reads (docs/try-tessaro.md, "The QEMU runtime").
set -eu
[ "$(uname -s)" = Darwin ] || { echo "macOS is required" >&2; exit 1; }
out=${1:?usage: bundle-qemu-macos.sh OUT_DIR}
here=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
prefix=$(brew --prefix qemu)
[ -x "$prefix/bin/qemu-system-aarch64" ] || { echo "needs qemu: brew install qemu" >&2; exit 1; }

rm -rf "$out"
mkdir -p "$out/bin" "$out/lib" "$out/share/qemu" "$out/LICENSES"
cp "$prefix/bin/qemu-system-aarch64" "$prefix/bin/qemu-img" "$out/bin/"
cp "$prefix/share/qemu/edk2-aarch64-code.fd" "$prefix/share/qemu/edk2-licenses.txt" "$out/share/qemu/"
chmod u+w "$out"/bin/*

# The dylibs a Mach-O file loads that macOS does not carry: the name the
# load command uses (a symlink's, libzstd.1.dylib) and the file it is.
# Homebrew's bottles link by absolute path; @loader_path and @rpath are
# resolved against the loading file's own directory.
deps() {
    dir=$(dirname "$(realpath "$1")")
    otool -L "$1" | tail -n +2 | awk '{print $1}' | while read -r lib; do
        case "$lib" in
        /usr/lib/* | /System/* | @executable_path/*) ;;
        @loader_path/* | @rpath/*) echo "$(basename "$lib") $(realpath "$dir/${lib#*/}")" ;;
        *) echo "$(basename "$lib") $(realpath "$lib")" ;;
        esac
    done
}

# Breadth first until no new dylib turns up.
queue="$out/bin/qemu-system-aarch64 $out/bin/qemu-img"
seen=" "
while [ -n "$queue" ]; do
    next=
    for file in $queue; do
        deps "$file" >"$out/.deps"
        while read -r name lib; do
            case "$seen" in *" $name "*) continue ;; esac
            seen="$seen$name "
            cp -L "$lib" "$out/lib/$name"
            chmod u+w "$out/lib/$name"
            echo "$lib" >>"$out/.sources"
            next="$next $out/lib/$name"
        done <"$out/.deps"
    done
    queue=$next
done
rm -f "$out/.deps"

# Every load command that names a copied dylib now points into lib/.
for file in "$out"/bin/* "$out"/lib/*.dylib; do
    [ -f "$file" ] || continue
    case "$file" in *.dylib) install_name_tool -id "@executable_path/../lib/$(basename "$file")" "$file" 2>/dev/null ;; esac
    otool -L "$file" | tail -n +2 | awk '{print $1}' | while read -r lib; do
        name=$(basename "$lib")
        if [ -f "$out/lib/$name" ] && [ "$lib" != "@executable_path/../lib/$name" ]; then
            install_name_tool -change "$lib" "@executable_path/../lib/$name" "$file" 2>/dev/null
        fi
    done
done

# Nothing may still point outside the bundle or the system.
leaks=$(for file in "$out"/bin/* "$out"/lib/*.dylib; do
    otool -L "$file" | tail -n +2 | awk '{print $1}'
done | grep -v -e '^/usr/lib/' -e '^/System/' -e '^@executable_path/' || true)
[ -z "$leaks" ] || { echo "still linked outside the bundle:" >&2; echo "$leaks" >&2; exit 1; }

# The formula each dylib came from: /opt/homebrew/Cellar/<formula>/<version>/.
formulas=$(sed -n 's|.*/Cellar/\([^/]*\)/.*|\1|p' "$out/.sources" | sort -u)
rm "$out/.sources"
brew info --json=v2 --formula qemu $formulas >"$out/runtime.json"
for formula in qemu $formulas; do
    cellar=$(brew --prefix "$formula")
    mkdir -p "$out/LICENSES/$formula"
    find -L "$cellar" -maxdepth 1 -type f \( -iname 'COPYING*' -o -iname 'LICENSE*' -o -iname 'LICENCE*' \) \
        -exec cp {} "$out/LICENSES/$formula/" \;
    # Some bottles (glib, dtc) keep no license file: name it and where the
    # source is, from the formula.
    if [ -z "$(ls "$out/LICENSES/$formula")" ]; then
        brew info --formula "$formula" | grep -E '^(==> |https?://|License:)' >"$out/LICENSES/$formula/NOTICE"
    fi
done
cp "$out/share/qemu/edk2-licenses.txt" "$out/LICENSES/qemu/"

# Relinking broke the signatures: dylibs ad hoc, QEMU with the hypervisor
# entitlement HVF needs, which an ad hoc signature can carry.
codesign --force --sign - "$out"/lib/*.dylib "$out/bin/qemu-img" >&2
codesign --force --sign - --entitlements "$here/qemu-hvf.entitlements" "$out/bin/qemu-system-aarch64" >&2
"$out/bin/qemu-system-aarch64" --version | head -n 1 >&2
