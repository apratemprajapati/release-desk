import { useEffect, useMemo, useState } from "react";
import { api, errText, Status } from "../lib/api";
import { Badge, Button, Field, inputCls } from "./ui";

const TYPES = ["feat", "fix", "perf", "refactor", "docs", "test", "build", "ci", "chore", "style", "revert"];

function Diff({ text }: { text: string }) {
  return (
    <pre className="h-full overflow-auto p-3 font-mono text-[12px] leading-5">
      {text.split("\n").map((l, i) => (
        <div
          key={i}
          className={
            l.startsWith("+++") || l.startsWith("---") ? "text-mute"
            : l.startsWith("+") ? "bg-ok/10 text-ok"
            : l.startsWith("-") ? "bg-bad/10 text-bad"
            : l.startsWith("@@") ? "text-accent" : "text-mute"
          }
        >
          {l || " "}
        </div>
      ))}
    </pre>
  );
}

/** Status + diff + staging + Conventional Commit editor. */
export function Changes(props: {
  projectId: number;
  busy: boolean;
  refreshKey: number;
  onCommit: (files: string[], message: string) => void;
}) {
  const [st, setSt] = useState<Status | null>(null);
  const [sel, setSel] = useState<Set<string>>(new Set());
  const [cur, setCur] = useState<string | null>(null);
  const [diff, setDiff] = useState("");
  const [err, setErr] = useState("");
  const [type, setType] = useState("feat");
  const [scope, setScope] = useState("");
  const [bang, setBang] = useState(false);
  const [desc, setDesc] = useState("");
  const [body, setBody] = useState("");

  useEffect(() => {
    api.gitStatus(props.projectId).then((s) => {
      setSt(s);
      setSel(new Set(s.files.map((f) => f.path)));
      setCur(s.files[0]?.path ?? null);
    }).catch((e) => setErr(errText(e)));
  }, [props.projectId, props.refreshKey]);

  useEffect(() => {
    if (!cur || !st) return setDiff("");
    const f = st.files.find((x) => x.path === cur);
    api.gitDiff(props.projectId, cur, f?.code === "??").then(setDiff).catch((e) => setDiff(errText(e)));
  }, [cur, st]);

  const message = useMemo(() => {
    const head = `${type}${scope.trim() ? `(${scope.trim()})` : ""}${bang ? "!" : ""}: ${desc.trim()}`;
    return body.trim() ? `${head}\n\n${body.trim()}\n` : `${head}\n`;
  }, [type, scope, bang, desc, body]);

  const toggle = (p: string) => setSel((s) => { const n = new Set(s); n.has(p) ? n.delete(p) : n.add(p); return n; });

  if (err) return <p className="text-bad">{err}</p>;
  if (!st) return <p className="text-mute">Reading status…</p>;

  return (
    <div className="flex h-full flex-col gap-3">
      <div className="flex items-center gap-3 text-[13px]">
        <span className="font-mono">{st.branch}</span>
        {st.upstream && <span className="text-mute">→ {st.upstream}</span>}
        <Badge tone={st.ahead ? "accent" : "mute"}>↑ {st.ahead} ahead</Badge>
        <Badge tone={st.behind ? "warn" : "mute"}>↓ {st.behind} behind</Badge>
      </div>
      {st.files.length === 0 ? (
        <p className="text-mute">Working tree is clean. Nothing to commit, so continue to Push or Version.</p>
      ) : (
        <>
          <div className="grid min-h-0 flex-1 grid-cols-[280px_1fr] overflow-hidden rounded-md border border-line">
            <div className="overflow-auto border-r border-line bg-bg">
              <label className="flex items-center gap-2 border-b border-line px-2 py-1.5 text-xs text-mute">
                <input type="checkbox" checked={sel.size === st.files.length}
                  onChange={(e) => setSel(e.target.checked ? new Set(st.files.map((f) => f.path)) : new Set())} />
                {sel.size}/{st.files.length} selected
              </label>
              {st.files.map((f) => (
                <div key={f.path}
                  className={`flex cursor-pointer items-center gap-2 px-2 py-1 text-[12px] ${cur === f.path ? "bg-panel2" : "hover:bg-panel"}`}
                  onClick={() => setCur(f.path)}>
                  <input type="checkbox" checked={sel.has(f.path)} onClick={(e) => e.stopPropagation()} onChange={() => toggle(f.path)} />
                  <span className="w-5 font-mono text-warn">{f.code.replace(/\./g, "").slice(0, 2) || "M"}</span>
                  <span className="truncate font-mono" title={f.path}>{f.path}</span>
                </div>
              ))}
            </div>
            <div className="min-w-0 bg-bg"><Diff text={diff} /></div>
          </div>
          <div className="grid grid-cols-[120px_160px_1fr_auto] items-end gap-2">
            <Field label="Type">
              <select className={inputCls} value={type} onChange={(e) => setType(e.target.value)}>
                {TYPES.map((t) => <option key={t}>{t}</option>)}
              </select>
            </Field>
            <Field label="Scope"><input className={inputCls} value={scope} onChange={(e) => setScope(e.target.value)} placeholder="optional" /></Field>
            <Field label="Description"><input className={inputCls} value={desc} onChange={(e) => setDesc(e.target.value)} placeholder="imperative, lower case" /></Field>
            <label className="flex items-center gap-1.5 pb-2 text-xs"><input type="checkbox" checked={bang} onChange={(e) => setBang(e.target.checked)} />breaking</label>
          </div>
          <textarea className={`${inputCls} h-16 font-mono`} placeholder="Body (optional). Add 'BREAKING CHANGE: …' for a major bump."
            value={body} onChange={(e) => setBody(e.target.value)} />
          <div className="flex items-center justify-between">
            <code className="truncate text-xs text-mute">{message.split("\n")[0]}</code>
            <Button variant="primary" disabled={props.busy || !sel.size || !desc.trim()} onClick={() => props.onCommit([...sel], message)}>
              Commit {sel.size} file{sel.size === 1 ? "" : "s"}
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
