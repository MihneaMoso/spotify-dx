# AGENTS.md — Spotify DX

Guidance for AI coding agents working in this repository.

**This file is the short pointer. The full reference lives in `RULES.md` — read it first and keep it up to date.**

## Ground rules

- **NEVER run, stop, restart, or kill `dx serve` (or the app binary) yourself.** The user runs the dev loop manually and has `dx serve` set to auto-reload. Starting/killing it wastes CPU cycles, clobbers the user's own instance, and risks duplicate servers. Let the user's running instance pick up changes on its own.
- **Do not touch build artifacts** under `target/`. The user's `dx serve` manages `target/dx`; leave it alone.
- **If something important changes, record it in `RULES.md`** so future agents don't repeat mistakes (see the "Discoveries & gotchas" section there).

## Quick reference

- Project: Rust + Dioxus desktop Spotify client (`dioxus 0.7`, `wry 0.53`, `dx` CLI 0.7.x).
- Dev loop: the user runs `dx serve` with auto-reload — do NOT start it yourself.
- Verify changes with `cargo check --features desktop` and `cargo clippy --features desktop` (do NOT run the app).
- Tests: `cargo test` (network-free) / `cargo test --no-default-features`.
- `SPOTIFY_CLIENT_ID` is read at **compile time** (`env!`) — builds will fail without it in the environment.

See `RULES.md` for the full agent-map, conventions, and gotchas.