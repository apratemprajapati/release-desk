import { useEffect, useState } from "react";
import { api, errText, Overview } from "../lib/api";
import { Badge, Button, Card } from "../components/ui";
import type { Route } from "../App";

export default function Dashboard({ go }: { go: (r: Route) => void }) {
  const [items, setItems] = useState<Overview[] | null>(null);
  const [err, setErr] = useState("");

  const load = () => api.dashboard().then(setItems).catch((e) => setErr(errText(e)));
  useEffect(() => {
    load();
    const t = setInterval(load, 15000);
    return () => clearInterval(t);
  }, []);

  return (
    <div className="p-6">
      <header className="mb-5 flex items-center justify-between">
        <h1 className="text-xl font-semibold">Projects</h1>
        <div className="flex gap-2">
          <Button onClick={load} title="Refresh">Refresh</Button>
          <Button variant="primary" onClick={() => go({ page: "add" })}>+ Add project</Button>
        </div>
      </header>
      {err && <p className="mb-3 text-bad">{err}</p>}
      {items && items.length === 0 && (
        <Card className="p-10 text-center">
          <p className="mb-3 text-mute">No projects yet. Add a local git repository to start.</p>
          <Button variant="primary" onClick={() => go({ page: "add" })}>Add project</Button>
        </Card>
      )}
      <div className="grid grid-cols-[repeat(auto-fill,minmax(320px,1fr))] gap-4">
        {items?.map((o) => (
          <Card key={o.project.id} className="flex flex-col p-4">
            <div className="mb-1 flex items-start justify-between gap-2">
              <div className="min-w-0">
                <div className="truncate text-[15px] font-semibold">{o.project.name}</div>
                <div className="truncate text-xs text-mute">
                  {o.project.owner ? `${o.project.owner}/${o.project.repo}` : "no GitHub remote"}
                </div>
              </div>
              <div className="flex shrink-0 gap-1">
                {o.project.local_only && <Badge tone="warn">local-only</Badge>}
                {o.path_missing ? <Badge tone="bad">missing</Badge> : o.dirty ? <Badge tone="warn">{o.dirty} changed</Badge> : <Badge tone="ok">clean</Badge>}
              </div>
            </div>
            <dl className="my-3 grid grid-cols-2 gap-y-1.5 text-[13px]">
              <dt className="text-mute">Version</dt>
              <dd className="font-mono" title={o.version_error ?? ""}>{o.version ?? <span className="text-bad">unreadable</span>}</dd>
              <dt className="text-mute">Branch</dt>
              <dd className="font-mono">
                {o.branch || "-"}
                {(o.ahead > 0 || o.behind > 0) && <span className="ml-1 text-mute">↑{o.ahead} ↓{o.behind}</span>}
              </dd>
              <dt className="text-mute">Last release</dt>
              <dd className="font-mono">{o.last_tag ?? "none"}</dd>
              <dt className="text-mute">Unreleased</dt>
              <dd>{o.unreleased} commit{o.unreleased === 1 ? "" : "s"}</dd>
            </dl>
            <div className="mt-auto flex gap-2">
              <Button variant="primary" disabled={o.path_missing} onClick={() => go({ page: "wizard", id: o.project.id })}>
                Release
              </Button>
              <Button onClick={() => go({ page: "history", id: o.project.id })}>History</Button>
              <Button onClick={() => go({ page: "edit", id: o.project.id })}>Edit</Button>
            </div>
          </Card>
        ))}
      </div>
    </div>
  );
}
