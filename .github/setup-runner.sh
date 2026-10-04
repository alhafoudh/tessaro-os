#!/usr/bin/env bash
# Set up the self-hosted GitHub Actions runner that image.yml builds images on, on
# the build host, as a systemd service of the user who builds there.
# Safe to run again: it skips what is done and refreshes the service's PATH.
# How the runner is used, and why: docs/ci.md.
#
#   .github/setup-runner.sh --repo OWNER/NAME [--token TOKEN] [--name NAME]
#                           [--dir DIR] [--work DIR] [--cache DIR] [--sources DIR]
#   .github/setup-runner.sh --repo OWNER/NAME --remove [--token TOKEN]
#
# The token is the one GitHub shows under Settings, Actions, Runners, "New
# self-hosted runner" (or, for --remove, "Remove"). Without --token the script
# asks the gh CLI for one, which needs admin rights on the repository.
set -euo pipefail

repo=""
token="${RUNNER_TOKEN:-}"
name="$(hostname -s)-yocto"
dir="$HOME/actions-runner"
work=""
# The host's shared cache, which the checkouts' cache/ links to too, so CI
# and workstation builds share downloads and sstate (docs/ci.md).
cache=/srv/tessaro/cache
# The store of the images' GPL, LGPL and AGPL sources, shared like the cache
# (docs/sbom.md, "Sources").
sources=/srv/tessaro/sources
remove=0

while [ $# -gt 0 ]; do
    case "$1" in
        --repo) repo="$2"; shift 2 ;;
        --token) token="$2"; shift 2 ;;
        --name) name="$2"; shift 2 ;;
        --dir) dir="$2"; shift 2 ;;
        --work) work="$2"; shift 2 ;;
        --cache) cache="$2"; shift 2 ;;
        --sources) sources="$2"; shift 2 ;;
        --remove) remove=1; shift ;;
        -h|--help) sed -n '2,15p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[ -n "$repo" ] || { echo "--repo OWNER/NAME is required" >&2; exit 2; }

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# A registration or removal token from gh, when none was given.
fetch_token() {
    command -v gh >/dev/null || die "no --token and no gh CLI: copy the token from the repository's Settings, Actions, Runners page"
    gh api -X POST "repos/$repo/actions/runners/$1" --jq .token
}

if [ "$remove" = 1 ]; then
    [ -d "$dir" ] || die "$dir does not exist"
    cd "$dir"
    say "Stopping and removing the service"
    if [ -f .service ]; then
        sudo ./svc.sh stop || true
        sudo ./svc.sh uninstall
    fi
    [ -n "$token" ] || token=$(fetch_token remove-token)
    say "Unregistering the runner"
    ./config.sh remove --token "$token"
    echo "The runner is unregistered. $dir (and its _work/ with the build trees) is left in place."
    exit 0
fi

say "Checking the host"
# What image:build and e2e:run need (DEVELOPMENT.md, Prerequisites).
missing=()
for cmd in curl tar jq docker kas-container mise zstd ruby bundle; do
    command -v "$cmd" >/dev/null || missing+=("$cmd")
done
[ ${#missing[@]} -eq 0 ] || die "missing on PATH: ${missing[*]}"
for group in docker kvm; do
    id -nG | tr ' ' '\n' | grep -qx "$group" || die "$(id -un) is not in the $group group"
done
[ -e /dev/kvm ] || die "/dev/kvm is missing: e2e needs KVM"
docker info >/dev/null 2>&1 || die "docker does not answer for $(id -un)"
[ -d "$cache" ] || die "$cache does not exist: pass --cache with the shared download and sstate cache"
cache="$(cd "$cache" && pwd -P)"
mkdir -p "$sources" || die "cannot create $sources: pass --sources with a directory this user can write"
sources="$(cd "$sources" && pwd -P)"

if [ ! -x "$dir/config.sh" ]; then
    say "Downloading the runner into $dir"
    version=$(curl -fsSL https://api.github.com/repos/actions/runner/releases/latest | jq -r .tag_name)
    version=${version#v}
    mkdir -p "$dir"
    curl -fsSL "https://github.com/actions/runner/releases/download/v$version/actions-runner-linux-x64-$version.tar.gz" \
        | tar -xz -C "$dir"
    # The runner's .NET needs a few system libraries; its script installs them.
    sudo "$dir/bin/installdependencies.sh"
fi
cd "$dir"

if [ ! -f .runner ]; then
    [ -n "$token" ] || token=$(fetch_token registration-token)
    say "Registering $name with github.com/$repo, label yocto"
    args=(--unattended --url "https://github.com/$repo" --token "$token"
          --name "$name" --labels yocto --replace)
    # The workspace holds a checkout with a build/ per machine, so it wants
    # the big disk.
    [ -z "$work" ] || { mkdir -p "$work"; args+=(--work "$work"); }
    ./config.sh "${args[@]}"
else
    say "Already registered ($(jq -r .agentName .runner 2>/dev/null || echo "$dir/.runner"))"
fi

# The service starts without the login shell's profile, so it gets the PATH
# of this shell's tools explicitly. Only the directories the jobs need.
say "Writing the service's PATH to $dir/.env"
path=""
for cmd in mise kas-container ruby bundle docker zstd; do
    d=$(dirname "$(command -v "$cmd")")
    case ":$path:" in *":$d:"*) ;; *) path="${path:+$path:}$d" ;; esac
done
path="$path:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
{
    echo "PATH=$path"
    echo "LANG=${LANG:-C.UTF-8}"
} > .env
cat .env

if [ ! -f .service ]; then
    say "Installing the service"
    sudo ./svc.sh install "$(id -un)"
    sudo ./svc.sh start
else
    say "Restarting the service, to pick up .env"
    sudo ./svc.sh stop
    sudo ./svc.sh start
fi
sudo ./svc.sh status | head -n 5 || true

for var in TESSARO_CACHE_DIR:"$cache" TESSARO_SOURCES_DIR:"$sources"; do
    key=${var%%:*}
    value=${var#*:}
    say "Repository variable $key"
    if command -v gh >/dev/null && gh variable set "$key" --repo "$repo" --body "$value" 2>/dev/null; then
        echo "set to $value"
    else
        echo "Set it by hand: Settings, Secrets and variables, Actions, Variables,"
        echo "  $key = $value"
    fi
done

echo
echo "Done. The runner shows as Idle under Settings, Actions, Runners."
