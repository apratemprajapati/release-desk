# Release Desk

Desktop app that runs the release flow for any number of local Git/GitHub projects:
status, commit, push, version bump, release notes, tag, draft and publish a GitHub release.
Tauri 2 (Rust) + React + TypeScript. Everything runs locally; the only network calls are
to the GitHub API and your projects' git remotes.

**Status: Phases 1 and 2** from `RELEASE_DESK_SPEC.md`. Phase 3 (self-hosted runner panel,
CHANGELOG.md, `release-desk.json`, AI notes, auto-update) is not built yet.

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
   repos you release, with *Contents*, *Pull requests* and *Actions* read/write (Metadata
   read is implied). Or use **Sign in with GitHub** (device flow): register a GitHub OAuth
   App once with "Enable Device Flow", paste its client ID, and approve the code in the
   browser. You can add several accounts and bind each project to one. The app checks
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

## Phase 2 features

Set these per project under **Edit project**:

- **Merge before release**: *Pull request* opens (or reuses) a PR from the current branch
  into the default branch, polls its check runs every 5 s, and merges with the chosen
  method, pinned to the commit that was checked. Then it fast-forwards your local default
  branch. *Local fast-forward* runs `git merge --ff-only` and works for local-only projects.
- **Release workflow** and **Trigger**: either the tag push starts it, or the app calls
  `workflow_dispatch` with inputs such as `version={version}` (`{tag}` and `{notes}` also work).
  The **Actions** step lists runs for the release commit with live job and step status,
  cancel, re-run failed jobs, and open in browser.
- **Attach artifacts**: downloads the chosen run's artifacts and uploads each as
  `<name>.zip` to the draft release. Existing assets are skipped.
- **Version files**: Cargo (`Cargo.toml` + `Cargo.lock`), Python (`pyproject.toml`
  `[project]` or `[tool.poetry]`), Tauri (`tauri.conf.json`), Android (`versionName`, and
  `versionCode` + 1 on every bump), on top of npm, plain file, regex and git tag.
- **Resume**: History shows **Resume** on failed or unfinished runs. The wizard reopens with
  the same version, notes, PR and draft release, at the first unfinished step.
- **Rollback**: before anything is pushed, *Undo bump* / *Roll back* deletes the local tag,
  drops the `chore(release)` commit with `git reset --soft HEAD~1`, and restores the version
  files. It refuses once the commit or tag is on the remote, and never force-pushes.

## Guardrails

- **Local-only** projects: push, GitHub release and publish are refused in the Rust core
  (`pipeline::guard_remote`), not just hidden in the UI. Commit and tag still work locally.
- **Dry run** goes through one gate (`exec::Ctx::side_effect`) for git writes, file writes
  and mutating HTTP calls. Tests assert the repo, remote and package files are unchanged
  and that the mock GitHub received zero POST/PUT/PATCH/DELETE.
- **No force**: the git wrapper panics on `--force`/`-f`.
- **Tokens**: OS keychain only. All logs, errors and audit records go through
  `redact::redact` (token patterns plus the exact token in use).
- **Secrets**: new untracked files start unticked. Files that look like keys, certificates or
  `.env` files are hidden in the diff view, and committing one needs an explicit confirmation
  (the backend refuses it otherwise).
- **Untrusted repo config**: `release-desk.json` cannot point version files or the changelog
  outside the repo, set the runner folder, pass a workflow path, or use branch or tag names
  that git would read as options. Owner, repo and tag names are validated before they go into
  GitHub API URLs. Artifact zips are capped at 1000 entries and 2 GiB.
- **Runners** started from the app are stopped when the app quits.
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
