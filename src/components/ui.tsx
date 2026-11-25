import { ReactNode, forwardRef, useEffect, useRef } from "react";

type BtnProps = React.ButtonHTMLAttributes<HTMLButtonElement> & { variant?: "primary" | "ghost" | "danger" };
export const Button = forwardRef<HTMLButtonElement, BtnProps>(function Button(
  { variant = "ghost", className = "", ...p },
  ref,
) {
  const v = {
    primary: "bg-accent text-white hover:brightness-110 border-accent",
    ghost: "bg-panel2 hover:bg-line border-line",
    danger: "bg-bad/15 text-bad hover:bg-bad/25 border-bad/40",
  }[variant];
  return (
    <button
      ref={ref}
      {...p}
      className={`inline-flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-[13px] font-medium disabled:opacity-40 disabled:cursor-not-allowed ${v} ${className}`}
    />
  );
});

export function Card({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <div className={`rounded-lg border border-line bg-panel ${className}`}>{children}</div>;
}

export function Badge({ tone = "mute", children }: { tone?: "ok" | "warn" | "bad" | "mute" | "accent"; children: ReactNode }) {
  const t = {
    ok: "text-ok border-ok/40 bg-ok/10",
    warn: "text-warn border-warn/40 bg-warn/10",
    bad: "text-bad border-bad/40 bg-bad/10",
    mute: "text-mute border-line bg-panel2",
    accent: "text-accent border-accent/40 bg-accent/10",
  }[tone];
  return <span className={`inline-flex items-center rounded border px-1.5 py-0.5 text-[11px] font-medium ${t}`}>{children}</span>;
}

export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="block">
      <span className="mb-1 block text-xs font-medium text-mute">{label}</span>
      {children}
      {hint && <span className="mt-1 block text-[11px] text-mute">{hint}</span>}
    </label>
  );
}

export const inputCls =
  "w-full rounded-md border border-line bg-bg px-2.5 py-1.5 text-[13px] outline-none focus:border-accent";

export function Log({ text }: { text: string }) {
  const ref = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
  }, [text]);
  return (
    <pre ref={ref} className="h-full overflow-auto whitespace-pre-wrap p-3 font-mono text-[12px] leading-5 text-mute">
      {text.split("\n").map((l, i) => (
        <div
          key={i}
          className={
            l.startsWith("ERROR") ? "text-bad" : l.includes("[dry-run]") ? "text-warn" : l.startsWith("$") ? "text-ink" : ""
          }
        >
          {l || " "}
        </div>
      ))}
    </pre>
  );
}

/** Confirmation for every remote action. Shows account and target repo. */
export function Confirm(props: {
  open: boolean;
  title: string;
  lines: [string, string][];
  dryRun: boolean;
  danger?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const btn = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!props.open) return;
    btn.current?.focus();
    const k = (e: KeyboardEvent) => e.key === "Escape" && props.onCancel();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [props.open]);
  if (!props.open) return null;
  return (
    <div className="fixed inset-0 z-50 grid place-items-center bg-black/60" onClick={props.onCancel}>
      <div role="dialog" aria-modal className="w-[460px] rounded-lg border border-line bg-panel p-5" onClick={(e) => e.stopPropagation()}>
        <div className="mb-1 flex items-center gap-2">
          <h2 className="text-base font-semibold">{props.title}</h2>
          {props.dryRun ? <Badge tone="warn">DRY RUN</Badge> : <Badge tone="bad">LIVE</Badge>}
        </div>
        <p className="mb-4 text-xs text-mute">
          {props.dryRun ? "Nothing will change. Commands are only listed in the log." : "This changes a remote. Check the account and repository."}
        </p>
        <dl className="mb-5 grid grid-cols-[110px_1fr] gap-y-1.5 text-[13px]">
          {props.lines.map(([k, v]) => (
            <div key={k} className="contents">
              <dt className="text-mute">{k}</dt>
              <dd className="font-mono break-all">{v}</dd>
            </div>
          ))}
        </dl>
        <div className="flex justify-end gap-2">
          <Button onClick={props.onCancel}>Cancel</Button>
          <Button ref={btn} variant={props.danger && !props.dryRun ? "danger" : "primary"} onClick={props.onConfirm}>
            {props.dryRun ? "Simulate" : "Confirm"}
          </Button>
        </div>
      </div>
    </div>
  );
}
