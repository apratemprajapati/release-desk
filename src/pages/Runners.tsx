import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { api, errText, GhRunner, LocalRunner, Project } from "../lib/api";
import { Badge, Button, Card, Field, inputCls, Log } from "../components/ui";

/** Self-hosted Actions runner panel: GitHub-side status plus start/stop of a local runner folder. */
export default function Runners() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [pid, setPid] = useState<number | null>(null);
  const [local, setLocal] = useState<LocalRunner | null>(null);
  const [remote, setRemote] = useState<GhRunner[] | null>(null);
  const [remoteErr, setRemoteErr] = useState("");
  const [log, setLog] = useState("");
  const [err, setErr] = useState("");

  useEffect(() => {
    api.listProjects().then((ps) => {
      setProjects(ps);
      setPid((cur) => cur ?? ps.find((p) => p.options?.runner_dir)?.id ?? ps[0]?.id ?? null);
    });
  }, []);

  const project = projects.find((p) => p.id === pid);

  useEffect(() => {
    if (!pid) return;
    let live = true;
    const tick = () => {
      api.runnerStatus(pid).then((s) => { if (live) { setLocal(s); setErr(""); } }).catch((e) => live && (setLocal(null), setErr(errText(e))));
      api.runnerLog(pid).then((l) => live && setLog(l)).catch(() => {});
    };
    const remoteTick = () =>
      api.githubRunners(pid).then((r) => { if (live) { setRemote(r); setRemoteErr(""); } }).catch((e) => live && setRemoteErr(errText(e)));
    tick();
    remoteTick();
    const a = setInterval(tick, 3000);
    const b = setInterval(remoteTick, 10000);
    return () => { live = false; clearInterval(a); clearInterval(b); };
  }, [pid]);

  async function act(kind: "start" | "stop") {
    if (!pid) return;
    if (kind === "stop" && local?.running && !(await ask("Stop the runner? A job it is running will fail.", { kind: "warning" }))) return;
    try {
      setLocal(kind === "start" ? await api.runnerStart(pid) : await api.runnerStop(pid));
      setErr("");
    } catch (e) {
      setErr(errText(e));
    }
  }

  return (
    <div className="max-w-4xl space-y-5 p-6">
      <header className="flex items-center justify-between">
        <h1 className="text-xl font-semibold">Self-hosted runners</h1>
        <div className="w-72">
          <Field label="Project">
            <select className={inputCls} value={pid ?? ""} onChange={(e) => setPid(+e.target.value)}>
              {projects.map((p) => <option key={p.id} value={p.id}>{p.name}</option>)}
            </select>
          </Field>
        </div>
      </header>

      <Card className="p-5">
        <h2 className="mb-3 font-semibold">This PC</h2>
        {!project?.options?.runner_dir ? (
          <p className="text-mute">No runner folder set. Add one under Edit project → Self-hosted runner folder.</p>
        ) : (
          <>
            <div className="mb-3 flex flex-wrap items-center gap-2 text-[13px]">
              <span className="font-mono">{project.options.runner_dir}</span>
              {local && (
                <>
                  {local.running ? <Badge tone="ok">running · pid {local.pid}</Badge> : <Badge tone="mute">stopped</Badge>}
                  {!local.exists && <Badge tone="bad">run script not found</Badge>}
                  {local.exists && !local.configured && <Badge tone="warn">not configured</Badge>}
                </>
              )}
            </div>
            {err && <p className="mb-3 text-bad">{err}</p>}
            <div className="mb-3 flex gap-2">
              <Button variant="primary" disabled={!local?.exists || !local.configured || local.running} onClick={() => act("start")}>Start</Button>
              <Button disabled={!local?.running} onClick={() => act("stop")}>Stop</Button>
            </div>
            {local?.exists && !local.configured && (
              <p className="mb-3 text-xs text-mute">
                Register it once from a terminal in that folder with <code className="font-mono">config.cmd --url https://github.com/{project.owner}/{project.repo} --token &lt;token&gt;</code>.
                The token comes from GitHub → repo Settings → Actions → Runners → New self-hosted runner.
              </p>
            )}
            <div className="h-56 rounded-md border border-line bg-bg"><Log text={log || "No runner output yet."} /></div>
          </>
        )}
      </Card>

      <Card className="p-5">
        <h2 className="mb-3 font-semibold">Registered on GitHub</h2>
        {remoteErr && <p className="text-warn">{remoteErr}</p>}
        {remote && !remote.length && <p className="text-mute">No self-hosted runners registered for this repo.</p>}
        {remote?.map((r) => (
          <div key={r.id} className="flex items-center gap-3 border-b border-line py-2 text-[13px] last:border-0">
            <Badge tone={r.status === "online" ? (r.busy ? "accent" : "ok") : "mute"}>{r.status === "online" ? (r.busy ? "busy" : "idle") : r.status}</Badge>
            <span className="font-medium">{r.name}</span>
            <span className="text-mute">{r.os}</span>
            <span className="ml-auto text-xs text-mute">{r.labels.map((l) => l.name).join(", ")}</span>
          </div>
        ))}
      </Card>
    </div>
  );
}
