# Superseded docs (kept for history, not maintained)

These describe the pre-migration world (Dioxus-rendered mobile app via the
`dx` generator, August–September 2026 proposals) and are **not** accurate for
the current codebase. The living docs are `../ARCHITECTURE.md` (behavioral
spec), `../KOTLIN_MIGRATION.md` (Android migration status), `../RESEARCH.md`
(source intel), and the repo-root `RULES.md` (conventions + gotchas).

| File | What it was | Why superseded |
|---|---|---|
| `PHASE4_HANDOFF.md` | Dioxus-era Phase 4 (cache fan-out, queue model) status, 2026-08-25 | Phases redefined by `KOTLIN_MIGRATION.md §14`; the work landed differently (owned Kotlin app) |
| `PLATFORM_PARITY.md` | Web/Android/iOS parity via `dx build` generated scaffolds | Android is now the owned Kotlin app in `android/` (`KOTLIN_MIGRATION.md`); no generator involved |
| `SYSTEM_DESIGN.md` | Rework proposal (not yet implemented), with `ROADMAP.md` phases | Implemented via the migration instead; code comments referencing `§6.x` below still resolve to this file's sections |
| `ROADMAP.md` | Groundwork TODO through Phase 0–X (all ✅ 2026-08) | Spent; migration phases live in `KOTLIN_MIGRATION.md §14` |
| `MASTER_PROMPT.md` | Fresh-agent spec: "Rust + Dioxus desktop rework" | Role/product description predates the Kotlin migration |

Code comments in `src/` still cite `SYSTEM_DESIGN.md §6.x` — read those as
`docs/old/SYSTEM_DESIGN.md §6.x`.
