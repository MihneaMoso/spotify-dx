#!/usr/bin/env bash
# Local Android cross-build verification for spotify-dx.
set -euo pipefail
cd "$(dirname "$0")/.."

TARGET="${ARCH:-aarch64-linux-android}"
LINK="${1:---check}"
PKG="com.spotifydx.app"

NDK_BIN=""
for cand in "${ANDROID_NDK_HOME:-}" "${NDK_HOME:-}" "${ANDROID_HOME:-}/ndk/"*/ "${ANDROID_SDK_ROOT:-}/ndk/"*/; do
  [ -z "$cand" ] && continue
  b="$cand/toolchains/llvm/prebuilt/linux-x86_64/bin"
  [ -x "$b/clang" ] && NDK_BIN="$b" && break
done
if [ -z "$NDK_BIN" ]; then
  echo "Android NDK toolchain not found (set ANDROID_NDK_HOME)." >&2
  exit 1
fi

case "$TARGET" in
  aarch64-linux-android) TRIPLE="aarch64-linux-android21" ;;
  *) echo "Unsupported target: $TARGET" >&2; exit 1 ;;
esac

workdir="$(mktemp -d)"
trap 'rm -rf "$workdir"' EXIT
cat > "$workdir/$TARGET-clang" <<EOF
#!/bin/bash
exec "$NDK_BIN/clang" --target=$TRIPLE "\$@"
EOF
chmod +x "$workdir/$TARGET-clang"

export PATH="$workdir:$PATH"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$workdir/$TARGET-clang"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_CC="$workdir/$TARGET-clang"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_AR="$NDK_BIN/llvm-ar"

echo "Verifying $TARGET ..."
if [ "$LINK" = "--build" ]; then
  cargo build --no-default-features --features mobile --target "$TARGET"
else
  cargo check --no-default-features --features mobile --target "$TARGET"
fi
echo "Android ($TARGET) cross-build OK."
