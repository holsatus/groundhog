#!/usr/bin/env bash
# Builds the Flatpak and installs it for the current user.
#
#   flatpak/build.sh           build, and install for this user
#   flatpak/build.sh bundle    also write flatpak/out/groundhog.flatpak, one
#                              file that installs on another machine
#
# Needs flatpak-builder: the distribution's package, or
#   flatpak install --user flathub org.flatpak.Builder
# The SDK and the Rust extension the manifest asks for are installed from
# Flathub on the first build. Everything the build writes goes to
# flatpak/out.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(dirname "$HERE")"
OUT="$HERE/out"
APP_ID=org.holsatus.Groundhog

if command -v flatpak-builder >/dev/null; then
    BUILDER=(flatpak-builder)
else
    # The sandboxed builder asks the host's flatpak which runtimes are
    # installed, and the host's would otherwise look for the user's
    # installation inside the sandbox's data directory, and find none.
    BUILDER=(flatpak run "--env=FLATPAK_USER_DIR=${FLATPAK_USER_DIR:-$HOME/.local/share/flatpak}"
        --command=flatpak-builder org.flatpak.Builder)
fi

# The host's flatpak installs what the build needs, where no installation
# (the user's or the system's) has it yet: flatpak-builder's own (sandboxed,
# when it is the Flathub one) may not see a per-user Flathub remote.
VERSION="$(sed -n "s/^runtime-version: '\(.*\)'$/\1/p" "$HERE/$APP_ID.yml")"
MISSING=()
for ref in Platform Sdk Sdk.Extension.rust-stable; do
    if ! flatpak info "org.freedesktop.$ref//$VERSION" >/dev/null 2>&1; then
        MISSING+=("org.freedesktop.$ref//$VERSION")
    fi
done
if [ ${#MISSING[@]} -gt 0 ]; then
    flatpak install --user --noninteractive flathub "${MISSING[@]}"
fi

# The sandboxed build has no network, so every crate in Cargo.lock is copied
# here first; `cargo vendor` prints the configuration that points Cargo at
# them, with the path relative to the workspace root.
mkdir -p "$OUT"
if ! (cd "$ROOT" && cargo vendor --locked flatpak/out/vendor \
    >flatpak/out/vendor-config.toml 2>flatpak/out/vendor.log); then
    cat "$OUT/vendor.log" >&2
    exit 1
fi
if ! grep -q replace-with "$OUT/vendor-config.toml"; then
    echo "error: cargo vendor printed no source replacement" >&2
    exit 1
fi

# Without rofiles-fuse, which guards installed files against a build that
# modifies them in place: the sandboxed builder often cannot reach FUSE, and
# this one module installs only fresh files.
"${BUILDER[@]}" --force-clean --disable-rofiles-fuse \
    --state-dir="$OUT/state" --repo="$OUT/repo" \
    "$OUT/app" "$HERE/$APP_ID.yml"

# The host's flatpak installs it, not the builder's: installing writes the
# path of the flatpak doing it into the launcher, and the sandboxed
# builder's (/app/bin/flatpak) does not exist outside its sandbox.
# Reinstalling replaces the previous build, launcher included.
flatpak remote-add --user --no-gpg-verify --if-not-exists groundhog-origin "$OUT/repo"
flatpak install --user --noninteractive --reinstall groundhog-origin "$APP_ID"

if [ "${1:-}" = bundle ]; then
    # The runtime comes from Flathub wherever the bundle is installed.
    flatpak build-bundle --runtime-repo=https://dl.flathub.org/repo/flathub.flatpakrepo \
        "$OUT/repo" "$OUT/groundhog.flatpak" "$APP_ID"
    echo "Wrote $OUT/groundhog.flatpak"
fi

echo "Run it with: flatpak run $APP_ID"
