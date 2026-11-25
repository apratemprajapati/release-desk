import { useCallback, useEffect, useState } from "react";
import { Account, api, Base, Bump, errText, Plan, Project, StepOutcome } from "../lib/api";
import { Badge, Button, Card, Confirm, inputCls, Log } from "../components/ui";
import { Changes } from "../components/Changes";
import { NotesEditor } from "../components/NotesEditor";
import type { Route } from "../App";

type Key = "changes" | "push" | "version" | "notes" | "tag" | "release" | "publish";
type State = "idle" | "running" | "ok" | "dry" | "fail" | "skip";

const STEPS: { key: Key; label: string; remote: boolean }[] = [
  { key: "changes", label: "Status & commit", remote: false },
  { key: "push", label: "Push", remote: true },
  { key: "version", label: "Version bump", remote: false },
  { key: "notes", label: "Release notes", remote: false },
  { key: "tag", label: "Release commit + tag", remote: true },
  { key: "release", label: "Draft release", remote: true },
  { key: "publish", label: "Publish", remote: true },
];

const ICON: Record<State, string> = { idle: "○", running: "⏳", ok: "✓", dry: "◌", fail: "✗", skip: "–" };
const TONE: Record<State, string> = { idle: "text-mute", running: "text-accent", ok: "text-ok", dry: "text-warn", fail: "text-bad", skip: "text-mute" };

export default function Wizard({ id, go }: { id: number; go: (r: Route) => void }) {
  const [project, setProject] = useState<Project | null>(null);
  const [dry, setDry] = useState(true);
  const [runId, setRunId] = useState<number | null>(null);
  const [account, setAccount] = useState<Account | null>(null);
  const [active, setActive] = useState<Key>("changes");
  const [state, setState] = useState<Record<Key, State>>(() => Object.fromEntries(STEPS.map((s) => [s.key, "idle"])) as Record<Key, State>);
  const [log, setLog] = useState("");
  const [busy, setBusy] = useState(false);
  const [refresh, setRefresh] = useState(0);
  const [bump, setBump] = useState<Bump>({ kind: "auto" });
  const [livePlan, setPlan] = useState<Plan | null>(null);
  // Once the version is written, the plan is frozen so a refresh cannot bump twice.
  const [frozen, setFrozen] = useState<Plan | null>(null);
  const plan = frozen ?? livePlan;
  const [planErr, setPlanErr] = useState("");
  const [notes, setNotes] = useState("");
  const [prerelease, setPrerelease] = useState(false);
  const [releaseId, setReleaseId] = useState<number | null>(null);
  const [releaseUrl, setReleaseUrl] = useState("");
  const [confirm, setConfirm] = useState<null | { title: string; lines: [string, string][]; run: () => void; danger?: boolean }>(null);

  useEffect(() => {
    api.listProjects().then((ps) => {
      const p = ps.find((x) => x.id === id) ?? null;
      setProject(p);
      if (p?.local_only) setState((s) => ({ ...s, push: "skip", release: "skip", publish: "skip" }));
    });
    api.accounts().then((as) => setAccount(as[0] ?? null));
  }, [id]);

  const loadPlan = useCallback((b: Bump, keepNotes = false) => {
    setPlanErr("");
    api.plan(id, b).then((p) => {
      setPlan(p);
      if (!keepNotes) setNotes(p.notes);
    }).catch((e) => { setPlan(null); setPlanErr(errText(e)); });
  }, [id]);
  useEffect(() => { loadPlan(bump); }, [bump, refresh]);

  const append = (title: string, o: StepOutcome | string) =>
    setLog((l) => `${l}${l ? "\n" : ""}── ${title} ${new Date().toLocaleTimeString()}\n${typeof o === "string" ? o : o.log}`);

  async function ensureRun(): Promise<number> {
    if (runId) return runId;
    const r = await api.startRun(id, dry);
    setRunId(r.run_id);
    if (r.account) setAccount(r.account);
    return r.run_id;
  }

  async function exec(key: Key, fn: (b: Base) => Promise<StepOutcome>, next?: Key) {
    setBusy(true);
    setState((s) => ({ ...s, [key]: "running" }));
    try {
      const rid = await ensureRun();
      const o = await fn({ runId: rid, id, dryRun: dry });
      append(STEPS.find((s) => s.key === key)!.label, o);
      setState((s) => ({ ...s, [key]: o.ok ? (dry ? "dry" : "ok") : "fail" }));
      if (o.ok) {
        if (!dry) setRefresh((n) => n + 1);
        if (next) setActive(next);
      }
      return o;
    } catch (e) {
      append(key, `ERROR: ${errText(e)}`);
      setState((s) => ({ ...s, [key]: "fail" }));
    } finally {
      setBusy(false);
    }
  }

  const target = project ? `${project.owner}/${project.repo}` : "";
  const acctLine: [string, string] = ["Account", account ? `${account.label} (@${account.github_login})` : "none configured"];
  const ask = (title: string, lines: [string, string][], run: () => void, danger = false) =>
    setConfirm({ title, lines: [acctLine, ["Repository", target], ...lines], run, danger });

  const skip = (k: Key, next: Key) => { setState((s) => ({ ...s, [k]: "skip" })); setActive(next); };

  const startOver = async () => {
    if (runId) await api.finishRun(runId, dry ? "dry-run" : "abandoned").catch(() => {});
    setRunId(null);
    setState(Object.fromEntries(STEPS.map((s) => [s.key, project?.local_only && STEPS.find((x) => x.key === s.key)!.remote && s.key !== "tag" ? "skip" : "idle"])) as Record<Key, State>);
    setReleaseId(null);
    setReleaseUrl("");
    setFrozen(null);
    setActive("changes");
    setLog("");
    setRefresh((n) => n + 1);
  };

  const finishLocal = async () => {
    if (runId) await api.finishRun(runId, dry ? "dry-run" : "done");
    append("Run", "finished (local-only: no remote steps)");
    go({ page: "history", id });
  };

  // Ctrl+Enter triggers the primary action of the current step via its button.
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.key === "Enter") { (document.querySelector("[data-primary]") as HTMLButtonElement | null)?.click(); }
      if (e.altKey && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
        const i = STEPS.findIndex((s) => s.key === active) + (e.key === "ArrowDown" ? 1 : -1);
        if (STEPS[i]) setActive(STEPS[i].key);
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [active]);

  if (!project) return <p className="p-6 text-mute">Loading…</p>;
  const locked = (k: Key) => project.local_only && STEPS.find((s) => s.key === k)!.remote && k !== "tag";

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-3 border-b border-line px-5 py-3">
        <Button onClick={() => go({ page: "dashboard" })}>←</Button>
        <div className="min-w-0">
          <div className="font-semibold">Release {project.name}</div>
          <div className="text-xs text-mute">{project.local_only ? "local-only" : target}{runId ? ` · run #${runId}` : ""}</div>
        </div>
        <div className="ml-auto flex items-center gap-3">
          <label className={`flex items-center gap-2 rounded-md border px-3 py-1.5 text-[13px] ${dry ? "border-warn/50 bg-warn/10 text-warn" : "border-bad/50 bg-bad/10 text-bad"}`}
            title={runId ? "Start over to change mode" : ""}>
            <input type="checkbox" checked={dry} disabled={!!runId} onChange={(e) => setDry(e.target.checked)} />
            Dry run {dry ? "(no changes)" : "OFF: live"}
          </label>
          <Button onClick={startOver}>Start over</Button>
        </div>
      </header>

      <div className="flex min-h-0 flex-1">
        <ol className="w-56 shrink-0 border-r border-line p-3">
          {STEPS.map((s, i) => (
            <li key={s.key}>
              <button onClick={() => setActive(s.key)}
                className={`mb-0.5 flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] ${active === s.key ? "bg-panel2" : "hover:bg-panel"}`}>
                <span className={`w-4 text-center ${TONE[state[s.key]]}`}>{ICON[state[s.key]]}</span>
                <span className={locked(s.key) ? "text-mute line-through" : ""}>{i + 1}. {s.label}</span>
              </button>
            </li>
          ))}
          <p className="mt-4 px-2 text-[11px] leading-4 text-mute">Ctrl+Enter runs the step. Alt+↑/↓ moves between steps.</p>
        </ol>

        <section className="min-w-0 flex-1 overflow-auto p-5">
          {active === "changes" && (
            <div className="h-full min-h-[420px]">
              <Changes projectId={id} busy={busy} refreshKey={refresh}
                onCommit={(files, message) => exec("changes", (b) => api.commit({ ...b, files, message }), "push")} />
              <div className="mt-2 flex justify-end"><Button onClick={() => skip("changes", "push")}>Skip</Button></div>
            </div>
          )}

          {active === "push" && (
            <StepBox title="Push branch" locked={locked("push")}>
              <p className="mb-4 text-mute">Runs <code className="font-mono">git push origin &lt;current branch&gt;</code> with your existing git credentials. Never forced.</p>
              <div className="flex gap-2">
                <Button data-primary variant="primary" disabled={busy || locked("push")}
                  onClick={() => ask("Push branch", [["Command", "git push origin <branch>"]], () => exec("push", api.push, "version"))}>Push…</Button>
                <Button onClick={() => skip("push", "version")}>Skip</Button>
              </div>
            </StepBox>
          )}

          {active === "version" && (
            <StepBox title="Version bump">
              <div className="mb-4 flex flex-wrap items-end gap-2">
                {(["auto", "patch", "minor", "major"] as const).map((k) => (
                  <Button key={k} disabled={!!frozen} variant={bump.kind === k ? "primary" : "ghost"} onClick={() => setBump({ kind: k })}>
                    {k === "auto" ? `Auto${plan ? ` (${plan.suggested_level})` : ""}` : k}
                  </Button>
                ))}
                <Button variant={bump.kind === "prerelease" ? "primary" : "ghost"} onClick={() => setBump({ kind: "prerelease", id: "rc" })}>prerelease</Button>
                {bump.kind === "prerelease" && (
                  <input className={`${inputCls} !w-24`} value={bump.id} onChange={(e) => setBump({ kind: "prerelease", id: e.target.value })} />
                )}
                <Button variant={bump.kind === "custom" ? "primary" : "ghost"} onClick={() => setBump({ kind: "custom", version: plan?.next ?? "" })}>custom</Button>
                {bump.kind === "custom" && (
                  <input className={`${inputCls} !w-32 font-mono`} value={bump.version} onChange={(e) => setBump({ kind: "custom", version: e.target.value })} />
                )}
              </div>
              {planErr && <p className="mb-3 text-bad">{planErr}</p>}
              {plan && (
                <>
                  <div className="mb-4 flex items-center gap-3 font-mono text-lg">
                    <span className="text-mute">{plan.current}</span><span>→</span><span className="text-ok">{plan.next}</span>
                    <Badge tone="accent">{plan.tag}</Badge>
                    {plan.tag_exists && <Badge tone="warn">tag exists</Badge>}
                  </div>
                  <div className="mb-3 text-xs text-mute">
                    Files: {plan.edits.length ? plan.edits.map((e) => e.path).join(", ") : "none (git-tag only)"} · {plan.commits.length} commits since {plan.prev_tag ?? "the start"}
                  </div>
                  <ul className="mb-4 max-h-48 overflow-auto rounded-md border border-line bg-bg p-2 font-mono text-[12px]">
                    {plan.commits.map((c) => (
                      <li key={c.sha} className="truncate">
                        <span className="text-mute">{c.sha.slice(0, 7)}</span>{" "}
                        <span className={c.breaking ? "text-bad" : c.kind === "feat" ? "text-ok" : c.kind === "fix" ? "text-accent" : "text-mute"}>
                          {c.kind ?? "-"}{c.breaking ? "!" : ""}
                        </span>{" "}{c.description}
                      </li>
                    ))}
                    {!plan.commits.length && <li className="text-mute">No commits since the last tag.</li>}
                  </ul>
                  <Button data-primary variant="primary" disabled={busy}
                    onClick={async () => {
                      const p = plan;
                      const o = await exec("version", (b) => api.version({ ...b, next: p.next }), "notes");
                      if (o?.ok) setFrozen(p);
                    }}>Write {plan.next}</Button>
                </>
              )}
            </StepBox>
          )}

          {active === "notes" && (
            <div className="flex h-full min-h-[420px] flex-col gap-3">
              <NotesEditor value={notes} onChange={setNotes} onRegenerate={() => loadPlan(bump)} />
              <div className="flex justify-end">
                <Button data-primary variant="primary" disabled={busy || !notes.trim()}
                  onClick={() => exec("notes", (b) => api.notes({ ...b, notes }), "tag")}>Use these notes</Button>
              </div>
            </div>
          )}

          {active === "tag" && plan && (
            <StepBox title="Release commit and tag">
              <ul className="mb-4 list-disc pl-5 text-mute">
                <li>Commit version files as <code className="font-mono text-ink">chore(release): {plan.tag}</code></li>
                <li>Create annotated tag <code className="font-mono text-ink">{plan.tag}</code> with the notes</li>
                {project.local_only ? <li className="text-warn">Local-only: nothing is pushed</li> : <li>Push the branch and the tag to origin</li>}
              </ul>
              {plan.tag_exists && <p className="mb-3 text-warn">Tag {plan.tag} already exists: this step only pushes it.</p>}
              <Button data-primary variant="primary" disabled={busy}
                onClick={() => {
                  const run = () => exec("tag", (b) => api.tag({ ...b, next: plan.next, notes }), project.local_only ? undefined : "release");
                  project.local_only ? run() : ask("Commit, tag and push", [["Tag", plan.tag], ["Pushes", "branch + tag"]], run);
                }}>
                {project.local_only ? "Commit and tag locally" : "Commit, tag and push…"}
              </Button>
              {project.local_only && state.tag !== "idle" && (
                <Button className="ml-2" onClick={finishLocal}>Finish run</Button>
              )}
            </StepBox>
          )}

          {active === "release" && plan && (
            <StepBox title="Draft GitHub release" locked={locked("release")}>
              <label className="mb-4 flex items-center gap-2">
                <input type="checkbox" checked={prerelease || plan.next.includes("-")} onChange={(e) => setPrerelease(e.target.checked)} />
                Mark as prerelease
              </label>
              <Button data-primary variant="primary" disabled={busy || locked("release")}
                onClick={() => ask("Create draft release", [["Tag", plan.tag], ["Request", `POST /repos/${target}/releases`]], async () => {
                  const o = await exec("release", (b) => api.release({ ...b, next: plan.next, notes, prerelease }), "publish");
                  if (o?.ok) { setReleaseId(o.meta.release_id ?? 0); setReleaseUrl(o.meta.url ?? ""); }
                })}>Create draft…</Button>
              {releaseUrl && <p className="mt-3 text-xs">Draft: <a className="text-accent" href={releaseUrl} target="_blank">{releaseUrl}</a></p>}
            </StepBox>
          )}

          {active === "publish" && plan && (
            <StepBox title="Publish release" locked={locked("publish")}>
              <p className="mb-4 text-mute">{releaseId === null ? "Create the draft first." : `Publishes draft #${releaseId || "(dry-run)"} for ${plan.tag}. Watchers are notified.`}</p>
              <Button data-primary variant="primary" disabled={busy || releaseId === null || locked("publish")}
                onClick={() => ask("Publish release", [["Tag", plan.tag], ["Request", `PATCH /repos/${target}/releases/${releaseId}`]],
                  () => exec("publish", (b) => api.publish({ ...b, releaseId: releaseId ?? 0 })), true)}>Publish…</Button>
            </StepBox>
          )}
          {(active === "tag" || active === "release" || active === "publish") && !plan && (
            <p className="text-mute">{planErr || "Computing version…"}</p>
          )}
        </section>
      </div>

      <div className="h-48 shrink-0 border-t border-line bg-panel">
        <Log text={log || "Step output appears here."} />
      </div>

      <Confirm open={!!confirm} title={confirm?.title ?? ""} lines={confirm?.lines ?? []} dryRun={dry} danger={confirm?.danger}
        onCancel={() => setConfirm(null)} onConfirm={() => { const r = confirm!.run; setConfirm(null); r(); }} />
    </div>
  );
}

function StepBox({ title, locked, children }: { title: string; locked?: boolean; children: React.ReactNode }) {
  return (
    <Card className="max-w-3xl p-5">
      <h2 className="mb-3 text-base font-semibold">{title}</h2>
      {locked ? <p className="text-warn">This project is local-only. This step is disabled here and refused by the backend.</p> : children}
    </Card>
  );
}

