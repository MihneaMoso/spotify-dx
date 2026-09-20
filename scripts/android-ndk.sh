#!/usr/bin/env bash
# Shared Android NDK toolchain setup (single source — sourced, not executed).
#
#   source "$(dirname "$0")/android-ndk.sh" [target]
#
# Discovers the NDK clang/llvm-ar, selects the API-30 sysroot triple (aligned
# with the app's minSdk 30), and exports the cargo wrapper env. Previously
# pasted into both build-kotlin.sh and check-android.sh, which drifted.
#
# Sets: NDK_BIN, TRIPLE, TARGET (+ exports PATH, CARGO_TARGET_*_*, CC_*, AR_*).
# Requires ANDROID_NDK_HOME / NDK_HOME / ANDROID_HOME / ANDROID_SDK_ROOT.
set -euo pipefail

TARGET="${1:-${ARCH:-aarch64-linux-android}}"

NDK_BIN=""
for cand in "${ANDROID_NDK_HOME:-}" "${NDK_HOME:-}" "${ANDROID_HOME:-}/ndk/"*/ "${ANDROID_SDK_ROOT:-}/ndk/"*/; do
  [ -z "$cand" ] && continue
  b="$cand/toolchains/llvm/prebuilt/linux-x86_64/bin"
  [ -x "$b/clang" ] && NDK_BIN="$b" && break
done
if [ -z "$NDK_BIN" ]; then
  echo "Android NDK toolchain not found (set ANDROID_NDK_HOME)." >&2
  return 1 2>/dev/null || exit 1
fi

case "$TARGET" in
  # Match min_sdk_version = 30 so link-time system stubs (notably
  # libaaudio.so) resolve from the selected Android API sysroot.
  aarch64-linux-android) TRIPLE="aarch64-linux-android30" ;;
  *) echo "Unsupported target: $TARGET" >&2; return 1 2>/dev/null || exit 1 ;;
esac

_ndk_workdir="$(mktemp -d)"
trap 'rm -rf "$_ndk_workdir"' EXIT
cat > "$_ndk_workdir/$TARGET-clang" <<EOF
#!/bin/bash
exec "$NDK_BIN/clang" --target=$TRIPLE "\$@"
EOF
chmod +x "$_ndk_workdir/$TARGET-clang"
# C build scripts (cc-rs, e.g. ring) probe `aarch64-linux-android-ar` on
# PATH — they do NOT read CARGO_TARGET_*_AR — and stock NDK bins only carry
# llvm-ar. Same wrapper trick as clang (this exact gap broke CI release).
cat > "$_ndk_workdir/$TARGET-ar" <<EOF
#!/bin/bash
exec "$NDK_BIN/llvm-ar" "\$@"
EOF
chmod +x "$_ndk_workdir/$TARGET-ar"

export PATH="$_ndk_workdir:$PATH"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$_ndk_workdir/$TARGET-clang"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_CC="$_ndk_workdir/$TARGET-clang"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_AR="$_ndk_workdir/$TARGET-ar"
export AR_aarch64_linux_android="$_ndk_workdir/$TARGET-ar"
export CC_aarch64_linux_android="$_ndk_workdir/$TARGET-clang"
