#!/usr/bin/env bash
#
# Manually trigger a CI release build without typing the tag + push by hand.
#
# It bumps/validates a semver tag, creates it on the current branch, pushes it
# to origin, and points you at the GitHub Actions run + release.
#
# Usage:
#   ./scripts/release.sh 0.1.1          # release version 0.1.1 (tag v0.1.1)
#   ./scripts/release.sh --dry-run 0.1.1  # show what would happen, don't do it
#
# Required: a committed git remote named `origin` (GitHub) and a working `gh`.

set -euo pipefail

# ── helpers ───────────────────────────────────────────────────────────────────
say()  { printf '\033[36m[release]\033[0m %s\n' "$*"; }
warn() { printf '\033[33m[release] ! %s\033[0m\n' "$*" >&2; }
die()  { printf '\033[31m[release] %s\033[0m\n' "$*" >&2; exit 1; }

DRY_RUN=0
case "${1:-}" in
  --dry-run) DRY_RUN=1; shift ;;
esac

VERSION="${1:-}"
[ -z "$VERSION" ] && die "usage: $0 [--dry-run] <version>  (e.g. $0 0.1.1)"
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  die "version must be semver like 0.1.1 (got '$VERSION')"
fi

TAG="v${VERSION}"
remote="${CI_RELEASE_REMOTE:-origin}"

# ── sanity checks (git repo, clean tree, remote reachable) ──────────────────
command -v git >/dev/null || die "git not found"
[ -d .git ] || die "must run from the repository root"

if [ -n "$(git status --porcelain)" ]; then
  warn "working tree is not clean — the tag will point at uncommitted changes"
  read -r -p "continue anyway? [y/N] " ans
  [[ "$ans" =~ ^[yY]$ ]] || die "aborted"
fi

git fetch "$remote" --tags >/dev/null 2>&1 || true
if git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null; then
  die "tag '${TAG}' already exists locally (or on $remote). Pick a new version or delete it."
fi

# ── plan ─────────────────────────────────────────────────────────────────────
say "release plan:"
say "  tag     : ${TAG}"
say "  remote  : ${remote}"
say "  workflow: .github/workflows/release.yml"
say "  │ will run: 'cargo build --release --features desktop' for each desktop target"
say "  │ then publish a GitHub Release 'spotify-dx ${TAG}'."
if [ "$DRY_RUN" = "1" ]; then
  say "dry-run: not creating or pushing anything."
  exit 0
fi

# ── commit the current release config if needed ─────────────────────────────
if [ -n "$(git status --porcelain -- .github scripts 2>/dev/null)" ]; then
  say "committing workflow/script changes so the tag build is reproducible..."
  git add .github scripts 2>/dev/null || true
  git commit --no-verify -m "ci: release workflow"
else
  say "workflow/script already committed."
fi

# ── create + push the tag (this is what triggers CI) ────────────────────────
say "creating tag ${TAG}…"
git tag -a "$TAG" -m "spotify-dx ${VERSION}"

say "pushing '${TAG}' to '${remote}' (triggers the GitHub release build)…"
git push "$remote" "$TAG"

# ── point the user at the run ────────────────────────────────────────────────
repo="$(git remote get-url "$remote" | sed -E 's#.*github.com[:/]##; s#\.git$##')"
say "pushed. CI release for ${TAG} is running:"
say "  https://github.com/${repo}/actions"
say "the GitHub Release (when done) will appear at:"
say "  https://github.com/${repo}/releases/tag/${TAG}"

# Optional: block until the workflow run completes if gh is available.
if command -v gh >/dev/null 2>&1 && [ -n "$repo" ]; then
  say "waiting for the release workflow to finish (Ctrl-C to skip)…"
  gh run watch --repo "$repo" --exit-status --run-id \
    "$(gh run list --repo "$repo" --workflow release.yml --branch "$TAG" --limit 1 --json databaseId --jq '.[0].databaseId')" \
    || warn "watch failed — check the run manually at ${repo}/actions"
fi
