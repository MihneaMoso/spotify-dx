#!/usr/bin/env bash
# Full verification gate (RULES §7 — run this before declaring any change
# done, and before pushing). Fails LOUDLY on the first red step; every
# step mirrors a real breakage class from the project history.
#
# Covers: Rust tests (both feature sets), clippy with warnings DENIED,
# wasm-target check (the Send-bound class CI caught, never local cargo),
# Kotlin compile, and both hand-rolled JVM suites (RangeServe +
# policy/TOTP/capture-JS regressions).
#
# Deliberately NOT covered (stay CI-only): `dx bundle` packaging layout,
# the desktop release matrix link, macOS/Windows builds, Pages deploy.
set -euo pipefail
cd "$(dirname "$0")/.."

run() {
    local name="$1"; shift
    echo "==> $name"
    if ! out=$("$@" 2>&1); then
        echo "$out" | tail -25
        echo "FAIL: $name"
        exit 1
    fi
    echo "$out" | grep -E "test result: ok|BUILD SUCCESSFUL|checks passed|Finished" | head -4
}

run "cargo test (default features)" cargo test
run "cargo test (headless)" cargo test --no-default-features
run "clippy, warnings denied (default)" cargo clippy --all-targets -- -D warnings
run "clippy, warnings denied (headless)" cargo clippy --no-default-features --all-targets -- -D warnings
run "wasm target check" cargo check --target wasm32-unknown-unknown --no-default-features --features web

run "kotlin compile (offline)" bash -c "cd android && ./gradlew compileDebugKotlin --offline --console=plain"

echo "==> JVM regression suites"
STDLIB=$(find ~/.gradle/caches -name "kotlin-stdlib-2*.jar" | head -1)
CP="android/app/build/tmp/kotlin-classes/debug:$STDLIB"
mkdir -p /tmp/opencode/rstest
javac -cp "$CP" -d /tmp/opencode/rstest \
    android/app/src/test/java/com/spotifydx/app/RangeServeTest.java \
    android/app/src/test/java/com/spotifydx/app/LogicRegressionTest.java || exit 1
java -cp "/tmp/opencode/rstest:$CP" com.spotifydx.app.RangeServeTest || exit 1
java -cp "/tmp/opencode/rstest:$CP" com.spotifydx.app.LogicRegressionTest || exit 1

echo "ALL CHECKS PASSED"
