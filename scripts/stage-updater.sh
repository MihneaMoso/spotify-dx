#!/usr/bin/env bash
# LEGACY dx-renderer path only (DX_LEGACY=1). The Kotlin app (android/, the
# primary build) carries SpotifyDxUpdater + SpotifyDxFileProvider as
# first-class source sets + manifest entries — nothing needs staging.
# This script remains for the dx-generated project until the Phase 7 cutover.
#
# Stage the in-repo Android self-update support (SpotifyDxUpdater +
# SpotifyDxFileProvider) into the dx-generated Android project. The APK
# self-update path needs a ContentProvider to serve the staged APK (avoiding
# FileUriExposedException) plus an intent launcher — both live in
# android/updater/ and are injected into the generated app module.
#
#   scripts/stage-updater.sh         # after `dx build --platform android` runs
#
# Idempotent: copies sources and patches the manifest with marker comments so
# re-runs do not duplicate the <provider> entry.
set -euo pipefail
cd "$(dirname "$0")/.."

SRC="android/updater/src/main"
APP="$(find target/dx app/target/dx -path '*/app/src/main' -type d 2>/dev/null | head -1 || true)"
if [ -z "$APP" ]; then
  echo "No dx Android project found yet (run a dx build/bundle first)." >&2
  exit 1
fi

PACKAGE_DIR="$APP/kotlin/com/spotifydx/app"
mkdir -p "$PACKAGE_DIR"
cp -n "$SRC/kotlin/com/spotifydx/app/SpotifyDxUpdater.kt" "$PACKAGE_DIR/SpotifyDxUpdater.kt"
cp -n "$SRC/kotlin/com/spotifydx/app/SpotifyDxFileProvider.kt" "$PACKAGE_DIR/SpotifyDxFileProvider.kt"

MANIFEST="$APP/AndroidManifest.xml"
python3 - "$MANIFEST" <<'PY'
import sys
path = sys.argv[1]
with open(path) as f:
    xml = f.read()

MARK = "<!-- spotify-dx-updater:provider -->"
if MARK not in xml:
    provider = (
        "        " + MARK + "\n"
        '        <provider android:name="com.spotifydx.app.SpotifyDxFileProvider"\n'
        '            android:authorities="com.spotifydx.app.updates"\n'
        '            android:exported="false"\n'
        '            android:grantUriPermissions="true" />\n'
    )
    xml = xml.replace("</application>", provider + "    </application>", 1)

with open(path, "w") as f:
    f.write(xml)
PY

echo "Updater staged into $APP"