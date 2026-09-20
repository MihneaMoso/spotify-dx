#!/usr/bin/env bash
# Build the Spotify DX Dioxus wasm web bundle and assemble a GitHub
# Pages-ready deploy directory. Used by the pages workflow (every master
# push). The release workflow packages its own tarball straight from
# `dx build` output (different artifact — versioned tars, not the Pages
# tree), so this script is NOT shared with release despite what older
# revisions of this header claimed.
#
#   scripts/build-web.sh            # assembles ./_deploy (site at root, app at /app)
#
# Requires: `dx` CLI on PATH and the wasm32-unknown-unknown target installed.
# Outputs the deploy directory to "$1" (default: ./_deploy):
#   site/ index.html...  -> served at /spotify-dx/     (landing page)
#   app/  index.html...  -> served at /spotify-dx/app/ (Dioxus wasm app)
set -euo pipefail
cd "$(dirname "$0")/.."

DEPLOY="${1:-_deploy}"
# Guard the destructive tree ops below: refuse empty, root, home, or
# outside-repo targets (a bad $1 must never delete outside the repo).
case "$DEPLOY" in
    "" | "/" | "$HOME" | ..) echo "refusing unsafe DEPLOY dir: '$DEPLOY'" >&2; exit 1 ;;
esac
case "$(realpath -m "$DEPLOY")" in
    "$(pwd)"/*) : ;;
    *) echo "refusing DEPLOY outside the repo: '$DEPLOY'" >&2; exit 1 ;;
esac
rm -rf "$DEPLOY" web/app
mkdir -p "$DEPLOY/app"

# 1. Build the app for the web. base_path = "spotify-dx/app" (Dioxus.toml) makes
#    the generated index.html reference /spotify-dx/app/*. --release gives a
#    smaller, faster wasm. dx writes the bundle under web/app/public/.
dx bundle --platform web --release -p spotify-dx --out-dir web/app

# 2. Flatten the dx output so the app files sit directly in web/app/
#    (web/app/index.html, web/app/wasm/...). Fail before deleting anything
#    else if dx changed its output layout (bundle vs build differ here).
test -d web/app/public || { echo "expected dx output at web/app/public/ — layout changed?" >&2; exit 1; }
shopt -s dotglob
mv web/app/public/* web/app/
rmdir web/app/public
shopt -u dotglob

# 3. Assemble the deploy tree. web/site is the landing page (relative URLs,
#    works at /spotify-dx/); web/app is the app (base_path "spotify-dx/app",
#    works at /spotify-dx/app/). .nojekyll stops GH Pages from running Jekyll
#    on the wasm files.
cp -rT web/site "$DEPLOY"
cp -rT web/app "$DEPLOY/app"
touch "$DEPLOY/.nojekyll"

echo "Web bundle assembled at $DEPLOY:"
find "$DEPLOY" -maxdepth 2 -type f | sort
