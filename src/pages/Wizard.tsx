import { useCallback, useEffect, useState } from "react";
import { ask as nativeAsk } from "@tauri-apps/plugin-dialog";
import { Account, api, Base, Bump, errText, openUrl, Plan, PrStatus, Project, StepOutcome, WorkflowRun } from "../lib/api";
import { Badge, Button, Card, Confirm, inputCls, Log } from "../components/ui";
import { Changes } from "../components/Changes";
import { NotesEditor } from "../components/NotesEditor";
import { ActionsMonitor } from "../components/ActionsMonitor";
import type { Route } from "../App";

type Key = "changes" | "push" | "merge" | "version" | "notes" | "tag" | "actions" | "release" | "publish";
type State = "idle" | "running" | "ok" | "dry" | "fail" | "skip";

const STEPS: { key: Key; label: string; remote: boolean }[] = [
  { key: "changes", label: "Status & commit", remote: false },
  { key: "push", label: "Push", remote: true },
  { key: "merge", label: "Merge", remote: false },
  { key: "version", label: "Version bump", remote: false },
  { key: "notes", label: "Release notes", remote: false },
  { key: "tag", label: "Release commit + tag", remote: false },
  { key: "actions", label: "Actions", remote: true },
  { key: "release", label: "Draft release", remote: true },
  { key: "publish", label: "Publish", remote: true },
];

/** Audit-log step keys → wizard steps (used when resuming). */
const FROM_LOG: Record<string, Key> = {
  commit: "changes", push: "push", pr: "merge", merge: "merge", version: "version", notes: "notes",
  tag: "tag", dispatch: "actions", release: "release", attach: "release", publish: "publish",
};

const ICON: Record<State, string> = { idle: "○", running: "⏳", ok: "✓", dry: "◌", fail: "✗", skip: "–" };
const TONE: Record<State, string> = { idle: "text-mute", running: "text-accent", ok: "text-ok", dry: "text-warn", fail: "text-bad", skip: "text-mute" };

const initialStates = (p: Project | null) =>
  Object.fromEntries(STEPS.map((s) => [s.key, p?.local_only && s.remote ? "skip" : "idle"])) as Record<Key, State>;

export default function Wizard({ id, resumeRunId, go }: { id: number; resumeRunId?: number; go: (r: Route) => void }) {
  const [project, setProject] = useState<Project | null>(null);
  const [dry, setDry] = useState(true);
  const [runId, setRunId] = useState<number | null>(null);
  const [account, setAccount] = useState<Account | null>(null);
  const [active, setActive] = useState<Key>("changes");
  const [state, setState] = useState<Record<Key, State>>(initialStates(null));
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
  const [prNumber, setPrNumber] = useState<number | null>(null);
  const [prStatus, setPrStatus] = useState<PrStatus | null>(null);
  const [allowFailing, setAllowFailing] = useState(false);
  const [actionsRun, setActionsRun] = useState<WorkflowRun | null>(null);
  const [actionsRunId, setActionsRunId] = useState<number | null>(null);
  const [repoConfig, setRepoConfig] = useState<string[]>([]);
  const [aiReady, setAiReady] = useState(false);
  const [confirm, setConfirm] = useState<null | { title: string; lines: [string, string][]; run: () => void; danger?: boolean }>(null);

  // ---------------------------------------------------------------- load / resume
  useEffect(() => {
    (async () => {
      // Effective settings: the repo's release-desk.json applied on top.
      const eff = await api.effectiveProject(id).catch(() => null);
      const p = eff?.project ?? null;
      if (eff) setRepoConfig(eff.repo_config);
      if (eff?.repo_config_error) append("release-desk.json", `ERROR: ${eff.repo_config_error}`);
      setProject(p);
      api.settings().then((s) => setAiReady(s.ai_key_set)).catch(() => {});
      setState(initialStates(p));
      const accts = await api.accounts();
      setAccount(accts.find((a) => a.id === p?.account_id) ?? accts[0] ?? null);
      if (!resumeRunId || !p) return;
      try {
        const info = await api.resumeInfo(resumeRunId);
        setRunId(info.run_id);
        setDry(info.dry_run);
        const st = initialStates(p);
        for (const [k, v] of Object.entries(info.steps)) {
          const key = FROM_LOG[k];
          if (key) st[key] = v === "failed" ? "fail" : v === "dry-run" ? "dry" : "ok";
        }
        setState(st);
        if (info.to_version && info.to_version !== info.from_version) {
          const fp = await api.plan(id, { kind: "custom", version: info.to_version });
          setFrozen(fp);
          setNotes(info.notes ?? fp.notes);
        } else if (info.notes) {
          setNotes(info.notes);
        }
        setPrNumber(info.pr_number);
        setReleaseId(info.release_id);
        setActionsRunId(info.actions_run_id);
        const firstOpen = STEPS.find((s) => st[s.key] === "fail" || st[s.key] === "idle");
        if (firstOpen) setActive(firstOpen.key);
        append("Resume", `resumed run #${info.run_id} (${info.from_version} → ${info.to_version}), was ${info.status}`);
      } catch (e) {
        append("Resume", `ERROR: ${errText(e)}`);
      }
    })();
  }, [id, resumeRunId]);

  const loadPlan = useCallback((b: Bump) => {
    setPlanErr("");
    api.plan(id, b).then((p) => { setPlan(p); setNotes((n) => (frozen ? n : p.notes)); })
      .catch((e) => { setPlan(null); setPlanErr(errText(e)); });
  }, [id, frozen]);
  useEffect(() => { if (!frozen) loadPlan(bump); }, [bump, refresh]);

  // Poll PR checks every 5 s while the merge step is open.
  useEffect(() => {
    if (active !== "merge" || !prNumber || !project || project.local_only) return;
    let live = true;
    const tick = () => api.prStatus(id, prNumber).then((s) => live && setPrStatus(s)).catch((e) => live && append("PR", `ERROR: ${errText(e)}`));
    tick();
    const t = setInterval(tick, 5000);
    return () => { live = false; clearInterval(t); };
  }, [active, prNumber]);

  const append = (title: string, o: StepOutcome | string) =>
    setLog((l) => `${l}${l ? "\n" : ""}── ${title} ${new Date().toLocaleTimeString()}\n${typeof o === "string" ? o : o.log}`);

  async function ensureRun(): Promise<number> {
    if (runId) return runId;
    const r = await api.startRun(id, dry);
    setRunId(r.run_id);
    if (r.account) setAccount(r.account);
    return r.run_id;
  }

  async function exec(key: Key, fn: (b: Base) => Promise<StepOutcome>, next?: Key, title?: string) {
    setBusy(true);
    setState((s) => ({ ...s, [key]: "running" }));
    try {
      const rid = await ensureRun();
      const o = await fn({ runId: rid, id, dryRun: dry });
      append(title ?? STEPS.find((s) => s.key === key)!.label, o);
      setState((s) => ({ ...s, [key]: o.ok ? (dry ? "dry" : "ok") : "fail" }));
      if (o.ok) {
        if (!dry) setRefresh((n) => n + 1);
        if (next) setActive(next);
      }
      return o;
    } catch (e) {
      append(title ?? key, `ERROR: ${errText(e)}`);
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
    setState(initialStates(project));
    setReleaseId(null); setReleaseUrl(""); setFrozen(null); setPrNumber(null); setPrStatus(null);
    setActionsRun(null); setActionsRunId(null);
    setActive("changes");
    setLog("");
    setRefresh((n) => n + 1);
  };

  const finishLocal = async () => {
    if (runId) await api.finishRun(runId, dry ? "dry-run" : "done");
    go({ page: "history", id });
  };

  const doRollback = () => {
    if (!plan) return;
    ask("Roll back local release", [["Undo", `tag ${plan.tag}, release commit, version files`], ["Safety", "refused if already pushed"]], async () => {
      const o = await exec("tag", (b) => api.rollback({ ...b, next: plan.next }), undefined, "Rollback");
      if (o?.ok && !dry) {
        setFrozen(null);
        setState((s) => ({ ...s, version: "idle", tag: "idle" }));
        setActive("version");
      }
    }, true);
  };

  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.key === "Enter") (document.querySelector("[data-primary]") as HTMLButtonElement | null)?.click();
      if (e.altKey && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
        const vis = STEPS.filter((s) => !project?.pipeline.length || (s.key === "changes" ? true : project.pipeline.includes(s.key)));
        const i = vis.findIndex((s) => s.key === active) + (e.key === "ArrowDown" ? 1 : -1);
        if (vis[i]) setActive(vis[i].key);
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [active]);

  if (!project) return <p className="p-6 text-mute">Loading…</p>;
  const o = project.options;
  const locked = (k: Key) => project.local_only && STEPS.find((s) => s.key === k)!.remote;
  const enabled = (k: Key) => !project.pipeline.length || (k === "changes"
    ? project.pipeline.includes("status") || project.pipeline.includes("commit")
    : project.pipeline.includes(k));
  const visible = STEPS.filter((s) => enabled(s.key));
  const needPlan = (k: Key) => ["tag", "actions", "release", "publish"].includes(k) && !plan;

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-3 border-b border-line px-5 py-3">
        <Button onClick={() => go({ page: "dashboard" })}>←</Button>
        <div className="min-w-0">
          <div className="font-semibold">Release {project.name}</div>
          <div className="text-xs text-mute">
            {project.local_only ? "local-only" : `${target} · ${account ? `@${account.github_login}` : "no account"}`}
            {runId ? ` · run #${runId}` : ""}
          </div>
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
          {visible.map((s, i) => (
            <li key={s.key}>
              <button onClick={() => setActive(s.key)}
                className={`mb-0.5 flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] ${active === s.key ? "bg-panel2" : "hover:bg-panel"}`}>
                <span className={`w-4 text-center ${TONE[state[s.key]]}`}>{ICON[state[s.key]]}</span>
                <span className={locked(s.key) ? "text-mute line-through" : ""}>{i + 1}. {s.label}</span>
              </button>
            </li>
          ))}
          <p className="mt-4 px-2 text-[11px] leading-4 text-mute">Ctrl+Enter runs the step. Alt+↑/↓ moves between steps.</p>
          {repoConfig.length > 0 && (
            <p className="mt-3 px-2 text-[11px] leading-4 text-accent" title={repoConfig.join(", ")}>
              release-desk.json applies {repoConfig.length} setting{repoConfig.length === 1 ? "" : "s"}
            </p>
          )}
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
                <Button data-primary variant="primary" disabled={busy}
                  onClick={() => ask("Push branch", [["Command", "git push origin <branch>"]], () => exec("push", api.push, "merge"))}>Push…</Button>
                <Button onClick={() => skip("push", "merge")}>Skip</Button>
              </div>
            </StepBox>
          )}

          {active === "merge" && (
            <StepBox title={`Merge into ${project.default_branch}`}>
              {o.merge_mode === "none" && (
                <>
                  <p className="mb-4 text-mute">Merging is off for this project: the release is made from the current branch. Turn it on under Edit project.</p>
                  <Button data-primary onClick={() => skip("merge", "version")}>Continue</Button>
                </>
              )}
              {o.merge_mode === "ff" && (
                <>
                  <p className="mb-4 text-mute">Local fast-forward: checks out <code className="font-mono">{project.default_branch}</code> and runs <code className="font-mono">git merge --ff-only</code> with the current branch. Nothing is pushed here.</p>
                  <div className="flex gap-2">
                    <Button data-primary variant="primary" disabled={busy}
                      onClick={() => exec("merge", api.mergeFf, "version")}>Fast-forward {project.default_branch}</Button>
                    <Button onClick={() => skip("merge", "version")}>Skip</Button>
                  </div>
                </>
              )}
              {o.merge_mode === "pr" && (locked("merge") ? (
                <p className="text-warn">Pull requests are disabled for local-only projects. Use the fast-forward merge mode instead.</p>
              ) : (
                <>
                  {!prNumber && (
                    <>
                      <p className="mb-4 text-mute">Opens a PR from the current branch into {project.default_branch} (or reuses an open one), waits for checks, then merges with <b>{o.merge_method}</b>.</p>
                      <div className="flex gap-2">
                        <Button data-primary variant="primary" disabled={busy}
                          onClick={() => ask("Open pull request", [["Into", project.default_branch], ["Request", `POST /repos/${target}/pulls`]], async () => {
                            const r = await exec("merge", (b) => api.openPr({ ...b, title: plan ? `Release ${plan.tag}` : "Release", body: notes }), undefined, "Open PR");
                            if (r?.ok) {
                              if (r.meta.pr) setPrNumber(r.meta.pr);
                              setState((s) => ({ ...s, merge: "idle" }));
                              if (dry) append("Open PR", "dry run: no PR exists, so checks and merge can only be simulated");
                            }
                          })}>Open PR…</Button>
                        {dry && <Button disabled={busy} onClick={() => exec("merge", (b) => api.mergePr({ ...b, number: 0, allowFailing: true }), "version", "Merge PR")}>Simulate merge</Button>}
                        <Button onClick={() => skip("merge", "version")}>Skip</Button>
                      </div>
                    </>
                  )}
                  {prNumber && (
                    <>
                      <div className="mb-3 flex items-center gap-2">
                        <span className="font-medium">PR #{prNumber}</span>
                        {prStatus && <Button className="!px-2 !py-0.5 text-xs" onClick={() => openUrl(prStatus.pr.html_url)}>Open</Button>}
                        {prStatus?.pr.merged && <Badge tone="ok">merged</Badge>}
                        {prStatus?.pr.mergeable === false && <Badge tone="bad">conflicts</Badge>}
                      </div>
                      {prStatus ? (
                        <>
                          <div className="mb-2 flex gap-2 text-xs">
                            <Badge tone="ok">{prStatus.checks.passed} passed</Badge>
                            <Badge tone={prStatus.checks.failed ? "bad" : "mute"}>{prStatus.checks.failed} failed</Badge>
                            <Badge tone={prStatus.checks.pending ? "accent" : "mute"}>{prStatus.checks.pending} pending</Badge>
                            <span className="text-mute">refreshing every 5 s</span>
                          </div>
                          <ul className="mb-4 max-h-48 overflow-auto rounded-md border border-line bg-bg p-2 text-[12px]">
                            {prStatus.checks.runs.map((c, i) => (
                              <li key={i} className="flex gap-2">
                                <span className={c.status !== "completed" ? "text-accent" : c.conclusion === "success" ? "text-ok" : "text-bad"}>
                                  {c.status !== "completed" ? "⏳" : c.conclusion === "success" ? "✓" : "✗"}
                                </span>{c.name}
                              </li>
                            ))}
                            {!prStatus.checks.total && <li className="text-mute">No checks reported for this commit.</li>}
                          </ul>
                        </>
                      ) : <p className="mb-3 text-mute">Loading checks…</p>}
                      <label className="mb-3 flex items-center gap-2 text-xs">
                        <input type="checkbox" checked={allowFailing} onChange={(e) => setAllowFailing(e.target.checked)} />
                        Merge even if checks are pending or failing
                      </label>
                      <Button data-primary variant="primary"
                        disabled={busy || (!allowFailing && !!prStatus && (prStatus.checks.pending > 0 || prStatus.checks.failed > 0))}
                        onClick={() => ask("Merge pull request", [["PR", `#${prNumber} → ${project.default_branch}`], ["Method", o.merge_method],
                          ["Then", `checkout ${project.default_branch}, fast-forward from origin`]],
                          () => exec("merge", (b) => api.mergePr({ ...b, number: prNumber, allowFailing }), "version", "Merge PR"), true)}>
                        Merge…
                      </Button>
                    </>
                  )}
                </>
              ))}
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
                <Button disabled={!!frozen} variant={bump.kind === "prerelease" ? "primary" : "ghost"} onClick={() => setBump({ kind: "prerelease", id: "rc" })}>prerelease</Button>
                {bump.kind === "prerelease" && !frozen && (
                  <select className={`${inputCls} !w-24`} value={bump.id} onChange={(e) => setBump({ kind: "prerelease", id: e.target.value })}>
                    {["alpha", "beta", "rc"].map((x) => <option key={x}>{x}</option>)}
                  </select>
                )}
                <Button disabled={!!frozen} variant={bump.kind === "custom" ? "primary" : "ghost"} onClick={() => setBump({ kind: "custom", version: plan?.next ?? "" })}>custom</Button>
                {bump.kind === "custom" && !frozen && (
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
                    {frozen && <Badge tone="mute">locked for this run</Badge>}
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
                  <div className="flex gap-2">
                    <Button data-primary variant="primary" disabled={busy}
                      onClick={async () => {
                        const p = plan;
                        const r = await exec("version", (b) => api.version({ ...b, next: p.next }), "notes");
                        if (r?.ok) setFrozen(p);
                      }}>Write {plan.next}</Button>
                    {frozen && state.tag !== "ok" && <Button variant="danger" disabled={busy} onClick={doRollback}>Undo bump…</Button>}
                  </div>
                </>
              )}
            </StepBox>
          )}

          {active === "notes" && (
            <div className="flex h-full min-h-[420px] flex-col gap-3">
              <NotesEditor value={notes} onChange={setNotes} onRegenerate={() => plan && api.plan(id, { kind: "custom", version: plan.next }).then((p) => setNotes(p.notes))}
                onPolish={aiReady ? async () => {
                  if (!(await nativeAsk("Send these release notes (text only) to the Anthropic API with your key to polish them?", { title: "Polish with AI", kind: "info" }))) return;
                  setBusy(true);
                  try { setNotes(await api.polishNotes(notes)); append("Notes", "polished with AI: review before using"); }
                  catch (e) { append("Notes", `ERROR: ${errText(e)}`); }
                  finally { setBusy(false); }
                } : undefined} polishing={busy} />
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
                {project.local_only ? <li className="text-warn">Local-only: nothing is pushed</li> : <li>Push the branch and the tag to origin{o.workflow && !o.dispatch ? ` (this triggers ${o.workflow} if it runs on tags)` : ""}</li>}
              </ul>
              {plan.tag_exists && <p className="mb-3 text-warn">Tag {plan.tag} already exists: this step only pushes it.</p>}
              <div className="flex gap-2">
                <Button data-primary variant="primary" disabled={busy}
                  onClick={() => {
                    const run = () => exec("tag", (b) => api.tag({ ...b, next: plan.next, notes }), project.local_only ? undefined : "actions");
                    project.local_only ? run() : ask("Commit, tag and push", [["Tag", plan.tag], ["Pushes", "branch + tag"]], run);
                  }}>
                  {project.local_only ? "Commit and tag locally" : "Commit, tag and push…"}
                </Button>
                {(state.tag === "fail" || (project.local_only && state.tag === "ok")) && (
                  <Button variant="danger" disabled={busy} onClick={doRollback}>Roll back…</Button>
                )}
                {project.local_only && state.tag !== "idle" && <Button onClick={finishLocal}>Finish run</Button>}
              </div>
              <p className="mt-3 text-[11px] text-mute">Roll back deletes the local tag, drops the release commit with a soft reset and restores the version files. It refuses once anything is on the remote and never force-pushes.</p>
            </StepBox>
          )}

          {active === "actions" && plan && (
            <StepBox title="GitHub Actions" locked={locked("actions")}>
              <div className="mb-4 flex flex-wrap items-center gap-2">
                {o.dispatch && o.workflow ? (
                  <Button data-primary variant="primary" disabled={busy}
                    onClick={() => ask("Trigger workflow", [["Workflow", o.workflow!], ["Ref", plan.tag],
                      ["Inputs", Object.entries(o.dispatch_inputs).map(([k, v]) => `${k}=${v}`).join(", ") || "none"]],
                      () => exec("actions", (b) => api.dispatch({ ...b, next: plan.next, notes }), undefined, "Dispatch"))}>
                    Run {o.workflow}…
                  </Button>
                ) : (
                  <span className="text-xs text-mute">
                    {o.workflow ? `${o.workflow} should start from the tag push.` : "No workflow set. Pick one under Edit project to dispatch it from here."}
                  </span>
                )}
                <Button onClick={() => skip("actions", "release")}>Skip</Button>
                {actionsRun && (
                  <Button variant="primary" onClick={() => { setActionsRunId(actionsRun.id); setState((s) => ({ ...s, actions: dry ? "dry" : "ok" })); setActive("release"); }}>
                    Use run #{actionsRun.run_number} →
                  </Button>
                )}
              </div>
              <ActionsMonitor projectId={id} gitRef={plan.tag_exists || state.tag === "ok" ? plan.tag : null} selected={actionsRun?.id ?? actionsRunId}
                onSelect={setActionsRun}
                onControl={(r, action) => ask(action === "cancel" ? "Cancel workflow run" : "Re-run failed jobs",
                  [["Run", `${r.name} #${r.run_number}`]],
                  () => exec("actions", (b) => api.actionsControl({ ...b, actionsRun: r.id, action }), undefined, `Actions ${action}`), action === "cancel")} />
            </StepBox>
          )}

          {active === "release" && plan && (
            <StepBox title="Draft GitHub release" locked={locked("release")}>
              <label className="mb-4 flex items-center gap-2">
                <input type="checkbox" checked={prerelease || plan.next.includes("-")} onChange={(e) => setPrerelease(e.target.checked)} />
                Mark as prerelease
              </label>
              <div className="flex flex-wrap gap-2">
                <Button data-primary variant="primary" disabled={busy}
                  onClick={() => ask("Create draft release", [["Tag", plan.tag], ["Request", `POST /repos/${target}/releases`]], async () => {
                    const r = await exec("release", (b) => api.release({ ...b, next: plan.next, notes, prerelease }));
                    if (r?.ok) { setReleaseId(r.meta.release_id ?? 0); setReleaseUrl(r.meta.url ?? ""); }
                  })}>{releaseId !== null ? "Update draft…" : "Create draft…"}</Button>
                {o.attach_artifacts && (
                  <Button disabled={busy || releaseId === null || !actionsRunId}
                    title={!actionsRunId ? "Pick a workflow run in the Actions step first" : ""}
                    onClick={() => ask("Attach build artifacts", [["From run", `${actionsRunId}`], ["To release", `${releaseId || "(dry-run)"}`]],
                      () => exec("release", (b) => api.attach({ ...b, releaseId: releaseId ?? 0, actionsRun: actionsRunId! }), undefined, "Attach artifacts"))}>
                    Attach artifacts…
                  </Button>
                )}
                {releaseId !== null && <Button onClick={() => setActive("publish")}>Next →</Button>}
              </div>
              {o.attach_artifacts && !actionsRunId && <p className="mt-3 text-xs text-mute">To attach build output, choose the finished run in the Actions step.</p>}
              {releaseUrl && <p className="mt-3 text-xs">Draft: <button className="text-accent" onClick={() => openUrl(releaseUrl)}>{releaseUrl}</button></p>}
            </StepBox>
          )}

          {active === "publish" && plan && (
            <StepBox title="Publish release" locked={locked("publish")}>
              <p className="mb-4 text-mute">{releaseId === null ? "Create the draft first." : `Publishes draft #${releaseId || "(dry-run)"} for ${plan.tag}. Watchers are notified.`}</p>
              <Button data-primary variant="primary" disabled={busy || releaseId === null}
                onClick={() => ask("Publish release", [["Tag", plan.tag], ["Request", `PATCH /repos/${target}/releases/${releaseId}`]],
                  () => exec("publish", (b) => api.publish({ ...b, releaseId: releaseId ?? 0 })), true)}>Publish…</Button>
            </StepBox>
          )}
          {needPlan(active) && <p className="text-mute">{planErr || "Computing version…"}</p>}
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
    <Card className="max-w-4xl p-5">
      <h2 className="mb-3 text-base font-semibold">{title}</h2>
      {locked ? <p className="text-warn">This project is local-only. This step is disabled here and refused by the backend.</p> : children}
    </Card>
  );
}
