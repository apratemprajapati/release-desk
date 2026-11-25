# Release Desk

Desktop app that runs the release flow for any number of local Git/GitHub projects:
status, commit, push, version bump, release notes, tag, draft and publish a GitHub release.
Tauri 2 (Rust) + React + TypeScript. Everything runs locally; the only network calls are
to the GitHub API and your projects' git remotes.

**Status: Phase 1 (MVP)** from `RELEASE_DESK_SPEC.md`. Phase 2 (PR merge, workflow
dispatch, run monitor, more adapters, multi-account, resume/rollback) is not built yet.

## Setup (Windows)

Prerequisites: [Rust](https://rustup.rs) (MSVC toolchain), Node 20+, Git for Windows,
and the WebView2 runtime (preinstalled on Windows 10/11).

```powershell
cd E:\pp\release-desk
npm install
npm run tauri dev          # run the app
npm run test:core          # Rust unit + integration tests (no GUI needed)
npm run tauri build        # MSI / NSIS installer in src-tauri\target\release\bundle
```

## Add a project

1. **Settings** (Ctrl+3): add a GitHub account. Paste a fine-grained token scoped to the
   repos you release, with *Contents: read/write* (Metadata read is implied). The app checks
   it against `GET /user`, stores it in Windows Credential Manager, and keeps only a
   reference in its database. Skip this if you only want local or dry runs.
2. **Projects → + Add project** (Ctrl+N), then **Browse…** and pick the repo folder.
   The app reads `origin` for owner/repo, the default branch, the version file
   (`package.json`, `VERSION`, otherwise git tags), and the workflows.
   - If the repo's `CLAUDE.md` forbids pushing (ytdwld does), or there is no GitHub
     remote, **Local-only** is switched on for you.
   - For other files, add a **regex** version source, e.g. file `src/app/__init__.py`,
     pattern `__version__ = "([^"]+)"` (one capture group around the version).
3. **Save**. The card shows version, branch, dirty files, last tag and unreleased commits.

## Do a dry-run release

1. Click **Release** on the card. **Dry run** is on by default (amber). It stays locked
   for the whole run; use **Start over** to change it.
2. Go through the steps in the left rail (Ctrl+Enter runs a step, Alt+Up/Down moves):
   - **Status & commit**: tick files, view diffs, write a Conventional Commit with the
     type/scope helper.
   - **Push**, **Version bump** (Auto suggests from commits: `feat` minor, `fix` patch,
     `!`/`BREAKING CHANGE` major; or pick patch/minor/major/prerelease/custom),
     **Release notes** (editable markdown with preview), **Release commit + tag**,
     **Draft release**, **Publish**.
   - Every remote step opens a confirmation that shows the account and `owner/repo`.
3. The log pane lists every command and API call that *would* run, as
   `[dry-run] would git push origin main`, `[dry-run] would POST /repos/o/r/releases …`.
   Nothing is written: no files, commits, tags, pushes or POST/PATCH requests.
   GitHub GET requests still run so the plan reflects reality.
4. **History** (Ctrl+2) keeps the run with each step's log and metadata.

Turn dry run off only for a throwaway test repo first (spec §9), e.g. one with the
`release.yml` from spec §10.

## Guardrails

- **Local-only** projects: push, GitHub release and publish are refused in the Rust core
  (`pipeline::guard_remote`), not just hidden in the UI. Commit and tag still work locally.
- **Dry run** goes through one gate (`exec::Ctx::side_effect`) for git writes, file writes
  and mutating HTTP calls. Tests assert the repo, remote and package files are unchanged
  and that the mock GitHub received zero POST/PUT/PATCH/DELETE.
- **No force**: the git wrapper panics on `--force`/`-f`.
- **Tokens**: OS keychain only. All logs, errors and audit records go through
  `redact::redact` (token patterns plus the exact token in use).
- **Idempotent**: tag step skips commit/tag if the tag exists; release step reuses an
  existing release for the tag.

## Layout

```
src-tauri/src/
  git.rs        git CLI wrapper, porcelain v2 parser
  github.rs     REST client (reqwest), releases
  version.rs    adapters (npm, plain-file, regex, git-tag), semver bump, tag formats
  notes.rs      Conventional Commit parser, notes renderer
  pipeline.rs   steps, local-only guard, detect, overview, plan
  store.rs      SQLite (accounts, projects, release_runs, run_steps), keychain
  exec.rs       dry-run gate and log capture
  redact.rs     secret redaction
  commands.rs   #[tauri::command] surface
src-tauri/tests/pipeline.rs   offline end-to-end: temp repo + bare remote + wiremock GitHub
src/            React UI (pages: Dashboard, AddProject, Wizard, History, Settings)
```

Deviations from the spec: `reqwest` instead of `octocrab` (smaller, easy to mock), and
small hand-written Tailwind components instead of shadcn/ui.
