import { useEffect, useState } from "react";
import { open, ask } from "@tauri-apps/plugin-dialog";
import { Account, Adapter, api, errText, Project } from "../lib/api";
import { Badge, Button, Card, Field, inputCls } from "../components/ui";
import type { Route } from "../App";

const blank: Project = {
  id: 0, name: "", path: "", owner: "", repo: "", account_id: null, default_branch: "main",
  release_branch: "main", tag_format: "v{version}", local_only: false, adapters: [], pipeline: [],
};

export default function AddProject({ go, editId }: { go: (r: Route) => void; editId?: number }) {
  const [p, setP] = useState<Project>(blank);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [note, setNote] = useState<string[]>([]);
  const [err, setErr] = useState("");
  const set = (patch: Partial<Project>) => setP((x) => ({ ...x, ...patch }));

  useEffect(() => {
    api.accounts().then(setAccounts);
    if (editId) api.listProjects().then((ps) => setP(ps.find((x) => x.id === editId) ?? blank));
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
      setNote(n);
      set({
        name: d.name, path: d.path, owner: d.owner, repo: d.repo, default_branch: d.default_branch,
        release_branch: d.default_branch, adapters: d.adapters,
        local_only: d.has_claude_md_push_ban || !d.owner,
      });
    } catch (e) {
      setErr(errText(e));
    }
  }

  async function save() {
    setErr("");
    try {
      if (editId) await api.updateProject(p);
      else await api.addProject(p);
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
                  {(["npm", "plain-file", "regex", "git-tag"] as const).map((k) => (
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
                    {(a.kind === "npm" || a.kind === "plain-file") && (
                      <input className={inputCls} placeholder={a.kind === "npm" ? "package.json" : "VERSION"}
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
