import { useEffect, useState } from "react";
import { api, Project, Run } from "../lib/api";
import { Badge, Button, Card, inputCls, Log } from "../components/ui";
import type { Route } from "../App";

const tone = (s: string) =>
  s === "done" || s === "ok" ? "ok" : s === "failed" ? "bad" : s === "dry-run" ? "warn" : s === "running" ? "accent" : "mute";

export default function History({ projectId, go }: { projectId?: number; go: (r: Route) => void }) {
  const [projects, setProjects] = useState<Project[]>([]);
  const [pid, setPid] = useState<number | null>(projectId ?? null);
  const [runs, setRuns] = useState<Run[]>([]);
  const [open, setOpen] = useState<number | null>(null);

  useEffect(() => { api.listProjects().then(setProjects); }, []);
  useEffect(() => { api.history(pid).then(setRuns); }, [pid]);
  const name = (id: number) => projects.find((p) => p.id === id)?.name ?? `#${id}`;

  return (
    <div className="p-6">
      <header className="mb-5 flex items-center justify-between">
        <h1 className="text-xl font-semibold">History</h1>
        <select className={`${inputCls} !w-56`} value={pid ?? ""} onChange={(e) => setPid(e.target.value ? +e.target.value : null)}>
          <option value="">All projects</option>
          {projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
        </select>
      </header>
      {!runs.length && <p className="text-mute">No release runs yet.</p>}
      <div className="space-y-2">
        {runs.map((r) => (
          <Card key={r.id}>
            <button className="flex w-full items-center gap-3 px-4 py-3 text-left" onClick={() => setOpen(open === r.id ? null : r.id)}>
              <span className="w-12 font-mono text-mute">#{r.id}</span>
              <span className="w-40 truncate font-medium">{name(r.project_id)}</span>
              <span className="font-mono">{r.from_version} → {r.to_version}</span>
              {r.dry_run && <Badge tone="warn">dry run</Badge>}
              <Badge tone={tone(r.status)}>{r.status}</Badge>
              <span className="ml-auto text-xs text-mute">{new Date(r.started_at).toLocaleString()}</span>
            </button>
            {(r.status === "failed" || r.status === "running") && (
              <div className="-mt-1 px-4 pb-3">
                <Button variant="primary" onClick={() => go({ page: "wizard", id: r.project_id, runId: r.id })}>
                  Resume run #{r.id}
                </Button>
              </div>
            )}
            {open === r.id && (
              <div className="border-t border-line">
                {r.steps.map((s) => (
                  <details key={s.id} className="border-b border-line last:border-0">
                    <summary className="flex cursor-pointer items-center gap-3 px-4 py-2 text-[13px]">
                      <span className="w-20 font-medium">{s.step_key}</span>
                      <Badge tone={tone(s.status)}>{s.status}</Badge>
                      <span className="ml-auto text-xs text-mute">{new Date(s.started_at).toLocaleTimeString()}</span>
                    </summary>
                    <div className="max-h-72 bg-bg"><Log text={s.log} /></div>
                    <pre className="overflow-auto bg-bg px-3 pb-3 font-mono text-[11px] text-mute">{prettify(s.meta_json)}</pre>
                  </details>
                ))}
                {!r.steps.length && <p className="px-4 py-3 text-mute">No steps recorded.</p>}
              </div>
            )}
          </Card>
        ))}
      </div>
    </div>
  );
}

function prettify(j: string) {
  try { return JSON.stringify(JSON.parse(j), null, 2); } catch { return j; }
}
