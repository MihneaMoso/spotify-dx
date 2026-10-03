# CI pre-commit check — proposal (pending review)

## Problem

Local checks cover host-target Rust + debug Kotlin only. CI additionally
covers wasm32, the desktop release matrix (linux/mac/win), Android release,
and the `dx` web bundle. Anything outside the local set can only break in
production — e.g. the wasm `Send`-bound breakage from the review, caught by
`pages.yml`, never by any local command.

## Proposal: `scripts/check-workflows.sh`

One local gate replicating every CI step that *can* run on Linux, with an
explicit gap table for what can't. Approximate runtime: a few minutes warm
(cargo/gradle incremental caches).

| CI job | Local equivalent in script | Gap (stays CI-only) |
|---|---|---|
| pages `dx bundle web` | `cargo check --target wasm32-unknown-unknown --no-default-features --features web`, plus `dx build --platform web` (`dx` is installed locally; closest mirror of the real breakage) | `dx bundle` packaging layout, Pages deploy |
| release `build` matrix (linux/mac/win × desktop) | `cargo check --target <each triple> --features desktop` (check needs no linker/SDK; script `rustup target add`s missing targets or warns) | actual linking (macOS SDK, MSVC), system-dep install |
| release `android-apk` | `./gradlew assembleDebug --offline` + bridge-compat check against staged `.so` | release signing (secrets), versioned upload |
| release `web` | covered by the wasm check above | tarball naming/upload |
| all jobs | `cargo test` (both feature sets), `cargo clippy` (warnings reported; `-D warnings` in script only, CI unchanged) | — |

## Drift rule

Any commit touching a workflow step must update the script in the same
commit (RULES.md review-checklist item). The script header carries the
coverage table above so staleness is visible at a glance.

## Belt and suspenders

Add a lightweight `check.yml` workflow (PR + push) running exactly this
script — a skipped local run still gets caught before master, without
touching the deploy workflows themselves.

## RULES.md change

Replace the don't-touch-workflows fear with: run
`scripts/check-workflows.sh` before committing anything under `src/`,
`android/`, `.github/workflows/`, `Cargo.*`, or DX config.

## Open questions for the reviewer

- Include the slower `dx build --platform web`, or is the wasm `cargo check` enough?
- Clippy deny-warnings in the script, or report-only?
- PR-gating `check.yml`: yes or no?
