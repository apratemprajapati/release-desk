import { invoke } from "@tauri-apps/api/core";

export type Adapter =
  | { kind: "npm"; file?: string | null }
  | { kind: "plain-file"; file?: string | null }
  | { kind: "regex"; file: string; pattern: string }
  | { kind: "git-tag" };

export interface Project {
  id: number;
  name: string;
  path: string;
  owner: string;
  repo: string;
  account_id: number | null;
  default_branch: string;
  release_branch: string;
  tag_format: string;
  local_only: boolean;
  adapters: Adapter[];
  pipeline: string[];
}

export interface Detected {
  name: string;
  path: string;
  owner: string;
  repo: string;
  remote_url: string | null;
  default_branch: string;
  adapters: Adapter[];
  has_claude_md_push_ban: boolean;
  workflows: string[];
}

export interface Overview {
  project: Project;
  version: string | null;
  version_error: string | null;
  branch: string;
  dirty: number;
  ahead: number;
  behind: number;
  last_tag: string | null;
  unreleased: number;
  path_missing: boolean;
}

export interface FileChange { path: string; code: string; staged: boolean }
export interface Status { branch: string; upstream: string | null; ahead: number; behind: number; files: FileChange[] }

export interface Commit {
  sha: string; author: string; kind: string | null; scope: string | null;
  breaking: boolean; description: string; pr: number | null;
}

export type Level = "none" | "patch" | "minor" | "major";
export type Bump =
  | { kind: "auto" } | { kind: "patch" } | { kind: "minor" } | { kind: "major" }
  | { kind: "prerelease"; id: string } | { kind: "custom"; version: string };

export interface Plan {
  current: string; next: string; suggested_level: Level; tag: string; prev_tag: string | null;
  tag_exists: boolean; commits: Commit[]; notes: string; edits: { path: string; content: string }[];
}

export interface StepOutcome { ok: boolean; log: string; meta: any }
export interface Account { id: number; label: string; github_login: string; keyring_ref: string }
export interface Step {
  id: number; step_key: string; status: string; log: string; meta_json: string;
  started_at: string; finished_at: string | null;
}
export interface Run {
  id: number; project_id: number; from_version: string; to_version: string; status: string;
  dry_run: boolean; started_at: string; finished_at: string | null; steps: Step[];
}
export interface Settings { git_path: string; default_tag_format: string }

export const api = {
  detectProject: (path: string) => invoke<Detected>("detect_project", { path }),
  addProject: (project: Project) => invoke<number>("add_project", { project }),
  updateProject: (project: Project) => invoke<void>("update_project", { project }),
  removeProject: (id: number) => invoke<void>("remove_project", { id }),
  listProjects: () => invoke<Project[]>("list_projects"),
  dashboard: () => invoke<Overview[]>("dashboard"),
  gitStatus: (id: number) => invoke<Status>("git_status", { id }),
  gitDiff: (id: number, path: string, untracked: boolean) => invoke<string>("git_diff", { id, path, untracked }),
  plan: (id: number, bump: Bump) => invoke<Plan>("release_plan", { id, bump }),
  startRun: (id: number, dryRun: boolean) =>
    invoke<{ run_id: number; account: Account | null }>("start_run", { id, dryRun }),
  finishRun: (runId: number, status: string) => invoke<void>("finish_run", { runId, status }),
  commit: (a: Base & { files: string[]; message: string }) => invoke<StepOutcome>("run_commit", a),
  push: (a: Base) => invoke<StepOutcome>("run_push", a),
  version: (a: Base & { next: string }) => invoke<StepOutcome>("run_version", a),
  notes: (a: Base & { notes: string }) => invoke<StepOutcome>("run_notes", a),
  tag: (a: Base & { next: string; notes: string }) => invoke<StepOutcome>("run_tag", a),
  release: (a: Base & { next: string; notes: string; prerelease: boolean }) => invoke<StepOutcome>("run_release", a),
  publish: (a: Base & { releaseId: number }) => invoke<StepOutcome>("run_publish", a),
  history: (projectId: number | null) => invoke<Run[]>("history", { projectId }),
  accounts: () => invoke<Account[]>("list_accounts"),
  addAccount: (label: string, token: string) => invoke<Account>("add_account", { label, token }),
  removeAccount: (id: number) => invoke<void>("remove_account", { id }),
  settings: () => invoke<Settings>("get_settings"),
  setSetting: (key: string, value: string) => invoke<void>("set_setting", { key, value }),
};

export type Base = { runId: number | null; id: number; dryRun: boolean };

export const errText = (e: unknown) => (typeof e === "string" ? e : (e as Error)?.message ?? String(e));
