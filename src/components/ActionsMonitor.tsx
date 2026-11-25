import { useEffect, useState } from "react";
import { api, errText, Job, openUrl, WorkflowRun } from "../lib/api";
import { Badge, Button } from "./ui";

const tone = (status: string | null, conclusion: string | null) =>
  status !== "completed" ? "accent" : conclusion === "success" ? "ok" : conclusion === "skipped" || conclusion === "neutral" ? "mute" : "bad";
const label = (status: string | null, conclusion: string | null) => (status === "completed" ? conclusion ?? "done" : status ?? "?");

/** Live list of workflow runs for the release commit, with job/step tree. Polls every 5 s. */
export function ActionsMonitor(props: {
  projectId: number;
  gitRef: string | null;
  selected: number | null;
  onSelect: (run: WorkflowRun) => void;
  onControl: (run: WorkflowRun, action: "cancel" | "rerun") => void;
}) {
  const [runs, setRuns] = useState<WorkflowRun[]>([]);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [err, setErr] = useState("");
  const [at, setAt] = useState("");

  useEffect(() => {
    let live = true;
    const tick = () =>
      api.actionsRuns(props.projectId, props.gitRef)
        .then((r) => { if (live) { setRuns(r); setErr(""); setAt(new Date().toLocaleTimeString()); } })
        .catch((e) => live && setErr(errText(e)));
    tick();
    const t = setInterval(tick, 5000);
    return () => { live = false; clearInterval(t); };
  }, [props.projectId, props.gitRef]);

  useEffect(() => {
    if (!props.selected) return setJobs([]);
    let live = true;
    const tick = () => api.actionsJobs(props.projectId, props.selected!).then((j) => live && setJobs(j)).catch(() => {});
    tick();
    const t = setInterval(tick, 5000);
    return () => { live = false; clearInterval(t); };
  }, [props.selected]);

  const sel = runs.find((r) => r.id === props.selected);

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between text-xs text-mute">
        <span>Runs for {props.gitRef ?? "HEAD"} · refreshed {at || "…"} (every 5 s)</span>
        {err && <span className="text-bad">{err}</span>}
      </div>
      {!runs.length && !err && <p className="text-mute">No runs for this commit yet. A run can take a few seconds to appear after the push or dispatch.</p>}
      <div className="overflow-hidden rounded-md border border-line">
        {runs.map((r) => (
          <button key={r.id} onClick={() => props.onSelect(r)}
            className={`flex w-full items-center gap-3 border-b border-line px-3 py-2 text-left text-[13px] last:border-0 ${props.selected === r.id ? "bg-panel2" : "hover:bg-panel"}`}>
            <Badge tone={tone(r.status, r.conclusion) as any}>{label(r.status, r.conclusion)}</Badge>
            <span className="font-medium">{r.name ?? "workflow"}</span>
            <span className="text-mute">#{r.run_number} · {r.event}</span>
            <span className="ml-auto text-xs text-mute">{new Date(r.created_at).toLocaleTimeString()}</span>
          </button>
        ))}
      </div>
      {sel && (
        <div className="rounded-md border border-line bg-bg p-3">
          <div className="mb-2 flex items-center gap-2">
            <span className="font-medium">{sel.name} #{sel.run_number}</span>
            <div className="ml-auto flex gap-2">
              <Button onClick={() => openUrl(sel.html_url)}>Open in browser</Button>
              <Button disabled={sel.status === "completed"} onClick={() => props.onControl(sel, "cancel")}>Cancel…</Button>
              <Button disabled={sel.status !== "completed" || sel.conclusion === "success"} onClick={() => props.onControl(sel, "rerun")}>Re-run failed…</Button>
            </div>
          </div>
          {jobs.map((j) => (
            <details key={j.id} open={j.status !== "completed" || j.conclusion !== "success"} className="mb-1">
              <summary className="flex cursor-pointer items-center gap-2 text-[13px]">
                <Badge tone={tone(j.status, j.conclusion) as any}>{label(j.status, j.conclusion)}</Badge>
                {j.name}
              </summary>
              <ol className="ml-6 mt-1 space-y-0.5 text-[12px]">
                {j.steps.map((s) => (
                  <li key={s.number} className="flex gap-2">
                    <span className={s.status !== "completed" ? "text-accent" : s.conclusion === "success" ? "text-ok" : s.conclusion === "skipped" ? "text-mute" : "text-bad"}>
                      {s.status !== "completed" ? "⏳" : s.conclusion === "success" ? "✓" : s.conclusion === "skipped" ? "–" : "✗"}
                    </span>
                    {s.name}
                  </li>
                ))}
              </ol>
            </details>
          ))}
        </div>
      )}
    </div>
  );
}
