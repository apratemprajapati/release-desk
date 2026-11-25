import { useEffect, useState } from "react";
import { open, ask } from "@tauri-apps/plugin-dialog";
import { Account, Adapter, ALL_STEPS, api, defaultOptions, errText, Project, ProjectOptions, TEMPLATES } from "../lib/api";
import { Badge, Button, Card, Field, inputCls } from "../components/ui";
import type { Route } from "../App";

const blank: Project = {
  id: 0, name: "", path: "", owner: "", repo: "", account_id: null, default_branch: "main",
  release_branch: "main", tag_format: "v{version}", local_only: false, adapters: [], pipeline: [],
  options: defaultOptions(),
};

const DEFAULT_FILE: Record<string, string> = {
  npm: "package.json", "plain-file": "VERSION", cargo: "Cargo.toml", python: "pyproject.toml",
  tauri: "src-tauri/tauri.conf.json", android: "app/build.gradle.kts",
};

export default function AddProject({ go, editId }: { go: (r: Route) => void; editId?: number }) {
  const [p, setP] = useState<Project>(blank);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [note, setNote] = useState<string[]>([]);
  const [err, setErr] = useState("");
  const [workflows, setWorkflows] = useState<string[]>([]);
  const [inputsText, setInputsText] = useState("version={version}");
  const [repoConfig, setRepoConfig] = useState<string[]>([]);
  const set = (patch: Partial<Project>) => setP((x) => ({ ...x, ...patch }));
  const setOpt = (patch: Partial<ProjectOptions>) => setP((x) => ({ ...x, options: { ...x.options, ...patch } }));
  const parseInputs = (t: string) =>
    Object.fromEntries(t.split("\n").map((l) => l.trim()).filter((l) => l.includes("=")).map((l) => [l.slice(0, l.indexOf("=")).trim(), l.slice(l.indexOf("=") + 1).trim()]));

  useEffect(() => {
    api.accounts().then(setAccounts);
    if (editId)
      api.listProjects().then((ps) => {
        const found = ps.find((x) => x.id === editId) ?? blank;
        const options = { ...defaultOptions(), ...found.options };
        setP({ ...found, options });
        setInputsText(Object.entries(options.dispatch_inputs).map(([k, v]) => `${k}=${v}`).join("\n"));
        api.detectProject(found.path).then((d) => setWorkflows(d.workflows)).catch(() => {});
        api.effectiveProject(found.id).then((e) => setRepoConfig(e.repo_config)).catch(() => {});
      });
    else api.settings().then((s) => set({ tag_format: s.default_tag_format }));
  }, [editId]);

  async function pick() {
    const dir = await open({ directory: true, title: "Choose a git repository folder" });
    if (!dir || Array.isArray(dir)) return;
    setErr("");
    try {
      const d = await api.detectProject(dir);
      const n: string[] = [];
      if (!d.remote_url) n.push("No 'origin' remote: the project is set to local-only.");
      else if (!d.owner) n.push(`Remote ${d.remote_url} is not on GitHub: releases will be unavailable.`);
      if (d.has_claude_md_push_ban) n.push("CLAUDE.md in this repo forbids pushing: local-only is switched on.");
      if (d.workflows.length) n.push(`Workflows found: ${d.workflows.join(", ")}`);
      setWorkflows(d.workflows);
      setNote(n);
      set({
        name: d.name, path: d.path, owner: d.owner, repo: d.repo, default_branch: d.default_branch,
        release_branch: d.default_branch, adapters: d.adapters,
        options: { ...defaultOptions(), workflow: d.workflows.find((w) => /release/i.test(w)) ?? null },
        local_only: d.has_claude_md_push_ban || !d.owner,
      });
    } catch (e) {
      setErr(errText(e));
    }
  }

  async function save() {
    setErr("");
    try {
      const full = { ...p, options: { ...p.options, dispatch_inputs: parseInputs(inputsText) } };
      if (editId) await api.updateProject(full);
      else await api.addProject(full);
      go({ page: "dashboard" });
    } catch (e) {
      setErr(errText(e));
    }
  }

  async function remove() {
    if (!(await ask(`Remove "${p.name}" from Release Desk? The repository folder is not touched; its release history is deleted.`, { kind: "warning" }))) return;
    await api.removeProject(p.id);
    go({ page: "dashboard" });
  }

  const setAdapter = (i: number, a: Adapter) => set({ adapters: p.adapters.map((x, j) => (j === i ? a : x)) });

  return (
    <div className="max-w-3xl p-6">
      <h1 className="mb-5 text-xl font-semibold">{editId ? `Edit ${p.name}` : "Add project"}</h1>
      <Card className="space-y-4 p-5">
        <Field label="Repository folder">
          <div className="flex gap-2">
            <input className={inputCls} value={p.path} readOnly placeholder="E:\pp\my-project" />
            {!editId && <Button variant="primary" onClick={pick}>Browse…</Button>}
          </div>
        </Field>
        {note.map((n) => (
          <p key={n} className="text-xs text-warn">{n}</p>
        ))}
        {p.path && (
          <>
            <div className="grid grid-cols-2 gap-4">
              <Field label="Name"><input className={inputCls} value={p.name} onChange={(e) => set({ name: e.target.value })} /></Field>
              <Field label="GitHub account">
                <select className={inputCls} value={p.account_id ?? ""} onChange={(e) => set({ account_id: e.target.value ? +e.target.value : null })}>
                  <option value="">{accounts[0] ? `Default (${accounts[0].label})` : "None configured"}</option>
                  {accounts.map((a) => <option key={a.id} value={a.id}>{a.label} ({a.github_login})</option>)}
                </select>
              </Field>
              <Field label="Owner"><input className={inputCls} value={p.owner} onChange={(e) => set({ owner: e.target.value })} /></Field>
              <Field label="Repo"><input className={inputCls} value={p.repo} onChange={(e) => set({ repo: e.target.value })} /></Field>
              <Field label="Default branch"><input className={inputCls} value={p.default_branch} onChange={(e) => set({ default_branch: e.target.value })} /></Field>
              <Field label="Release branch"><input className={inputCls} value={p.release_branch} onChange={(e) => set({ release_branch: e.target.value })} /></Field>
              <Field label="Tag format" hint="Must contain {version}, e.g. v{version}">
                <input className={inputCls} value={p.tag_format} onChange={(e) => set({ tag_format: e.target.value })} />
              </Field>
              <Field label="Remote access">
                <label className="flex items-center gap-2 py-1.5">
                  <input type="checkbox" checked={p.local_only} onChange={(e) => set({ local_only: e.target.checked })} />
                  <span>Local-only</span>
                  <span className="text-xs text-mute">(blocks push and releases)</span>
                </label>
              </Field>
            </div>

            <div>
              <div className="mb-2 flex items-center justify-between">
                <span className="text-xs font-medium text-mute">Version files (the first one is the source of truth)</span>
                <div className="flex gap-1">
                  {(["npm", "cargo", "python", "tauri", "android", "plain-file", "regex", "git-tag"] as const).map((k) => (
                    <Button key={k} className="!px-2 !py-0.5 text-xs" onClick={() =>
                      set({ adapters: [...p.adapters, k === "regex" ? { kind: k, file: "", pattern: "" } : ({ kind: k } as Adapter)] })
                    }>+ {k}</Button>
                  ))}
                </div>
              </div>
              <div className="space-y-2">
                {p.adapters.map((a, i) => (
                  <div key={i} className="flex items-center gap-2 rounded-md border border-line bg-bg p-2">
                    <Badge tone="accent">{a.kind}</Badge>
                    {a.kind !== "regex" && a.kind !== "git-tag" && (
                      <input className={inputCls} placeholder={DEFAULT_FILE[a.kind]}
                        value={a.file ?? ""} onChange={(e) => setAdapter(i, { ...a, file: e.target.value || null })} />
                    )}
                    {a.kind === "regex" && (
                      <>
                        <input className={inputCls} placeholder="src/version.py" value={a.file}
                          onChange={(e) => setAdapter(i, { ...a, file: e.target.value })} />
                        <input className={`${inputCls} font-mono`} placeholder={'__version__ = "([^"]+)"'} value={a.pattern}
                          onChange={(e) => setAdapter(i, { ...a, pattern: e.target.value })} />
                      </>
                    )}
                    {a.kind === "git-tag" && <span className="flex-1 text-xs text-mute">version comes from the last tag; no file is written</span>}
                    <Button className="!px-2" onClick={() => set({ adapters: p.adapters.filter((_, j) => j !== i) })} aria-label="Remove">✕</Button>
                  </div>
                ))}
              </div>
            </div>

            {repoConfig.length > 0 && (
              <p className="text-xs text-accent">
                This repo's release-desk.json overrides: {repoConfig.join(", ")}. Values below are the app's own settings.
              </p>
            )}
            <div className="grid grid-cols-2 gap-4 border-t border-line pt-4">
              <Field label="Template" hint="Fills the steps and options below; you can still change them">
                <select className={inputCls} value="" onChange={(e) => {
                  const t = TEMPLATES.find((x) => x.id === e.target.value);
                  if (t) { set({ pipeline: t.pipeline }); setOpt(t.options); }
                }}>
                  <option value="">Apply a template…</option>
                  {TEMPLATES.map((t) => <option key={t.id} value={t.id}>{t.label}</option>)}
                </select>
              </Field>
              <Field label="Steps">
                <div className="flex flex-wrap gap-x-3 gap-y-1 py-1 text-xs">
                  {ALL_STEPS.map((k) => (
                    <label key={k} className="flex items-center gap-1">
                      <input type="checkbox" checked={!p.pipeline.length || p.pipeline.includes(k)}
                        onChange={(e) => {
                          const cur = p.pipeline.length ? p.pipeline : [...ALL_STEPS];
                          set({ pipeline: e.target.checked ? ALL_STEPS.filter((x) => cur.includes(x) || x === k) : cur.filter((x) => x !== k) });
                        }} />
                      {k}
                    </label>
                  ))}
                </div>
              </Field>
              <Field label="Merge before release" hint="PR needs a GitHub account; fast-forward is local only">
                <select className={inputCls} value={p.options.merge_mode} onChange={(e) => setOpt({ merge_mode: e.target.value as any })}>
                  <option value="none">None: release from the current branch</option>
                  <option value="pr" disabled={p.local_only}>Pull request, wait for checks, merge</option>
                  <option value="ff">Local fast-forward into default branch</option>
                </select>
              </Field>
              <Field label="PR merge method">
                <select className={inputCls} value={p.options.merge_method} disabled={p.options.merge_mode !== "pr"}
                  onChange={(e) => setOpt({ merge_method: e.target.value as any })}>
                  <option value="squash">squash</option><option value="merge">merge commit</option><option value="rebase">rebase</option>
                </select>
              </Field>
              <Field label="Release workflow" hint="File in .github/workflows">
                <input className={inputCls} list="wf" value={p.options.workflow ?? ""} placeholder="release.yml"
                  onChange={(e) => setOpt({ workflow: e.target.value || null })} />
                <datalist id="wf">{workflows.map((w) => <option key={w} value={w} />)}</datalist>
              </Field>
              <Field label="Trigger">
                <select className={inputCls} value={p.options.dispatch ? "dispatch" : "tag"} onChange={(e) => setOpt({ dispatch: e.target.value === "dispatch" })}>
                  <option value="tag">Tag push starts the workflow</option>
                  <option value="dispatch">Call workflow_dispatch from the app</option>
                </select>
              </Field>
              {p.options.dispatch && (
                <Field label="Dispatch inputs (one name=value per line)" hint="Values can use {version}, {tag}, {notes}. Names must match the workflow's inputs.">
                  <textarea className={`${inputCls} h-16 font-mono`} value={inputsText} onChange={(e) => setInputsText(e.target.value)} />
                </Field>
              )}
              <Field label="Release assets">
                <select className={inputCls} value={p.options.attach_artifacts ? p.options.attach_mode : "off"}
                  onChange={(e) => e.target.value === "off" ? setOpt({ attach_artifacts: false }) : setOpt({ attach_artifacts: true, attach_mode: e.target.value as any })}>
                  <option value="files">Attach the files inside each artifact</option>
                  <option value="zip">Attach each artifact as a .zip</option>
                  <option value="off">Do not attach artifacts</option>
                </select>
              </Field>
              <Field label="Changelog">
                <label className="flex items-center gap-2 py-1">
                  <input type="checkbox" checked={p.options.changelog} onChange={(e) => setOpt({ changelog: e.target.checked })} />
                  <span>Add the notes to</span>
                  <input className="w-40" value={p.options.changelog_file} disabled={!p.options.changelog}
                    onChange={(e) => setOpt({ changelog_file: e.target.value })} />
                </label>
              </Field>
              <Field label="Self-hosted runner folder" hint="Optional: an actions-runner folder on this PC, managed from the Runners page">
                <div className="flex gap-2">
                  <input className={inputCls} value={p.options.runner_dir ?? ""} placeholder="C:\actions-runner"
                    onChange={(e) => setOpt({ runner_dir: e.target.value || null })} />
                  <Button onClick={async () => {
                    const d = await open({ directory: true, title: "Choose the actions-runner folder" });
                    if (d && !Array.isArray(d)) setOpt({ runner_dir: d });
                  }}>Browse…</Button>
                </div>
              </Field>
            </div>
          </>
        )}
        {err && <p className="text-bad">{err}</p>}
        <div className="flex justify-between pt-2">
          <div>{editId && <Button variant="danger" onClick={remove}>Remove project</Button>}</div>
          <div className="flex gap-2">
            <Button onClick={() => go({ page: "dashboard" })}>Cancel</Button>
            <Button variant="primary" disabled={!p.path || !p.adapters.length || !p.tag_format.includes("{version}")} onClick={save}>
              Save
            </Button>
          </div>
        </div>
      </Card>
    </div>
  );
}
