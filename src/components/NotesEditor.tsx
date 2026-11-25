import { marked } from "marked";
import { useMemo } from "react";
import { Button, inputCls } from "./ui";

/** Split markdown editor and preview. */
export function NotesEditor(props: {
  value: string; onChange: (v: string) => void; onRegenerate: () => void; onPolish?: () => void; polishing?: boolean;
}) {
  // Raw HTML in notes is escaped: commit messages are untrusted input.
  const html = useMemo(() => {
    const escaped = props.value.replace(/</g, "&lt;").replace(/>/g, "&gt;");
    const out = marked.parse(escaped, { async: false }) as string;
    // Only http(s) links survive; javascript: and friends are neutralised.
    return out.replace(/href="(?!https?:\/\/)[^"]*"/gi, 'href="#"').replace(/<a /g, '<a target="_blank" rel="noreferrer" ');
  }, [props.value]);
  return (
    <div className="flex h-full flex-col gap-2">
      <div className="flex justify-end gap-2">
        {props.onPolish && <Button disabled={props.polishing} onClick={props.onPolish}>{props.polishing ? "Polishing…" : "Polish with AI…"}</Button>}
        <Button onClick={props.onRegenerate}>Regenerate from commits</Button>
      </div>
      <div className="grid min-h-0 flex-1 grid-cols-2 gap-3">
        <textarea className={`${inputCls} h-full resize-none font-mono leading-5`} value={props.value}
          onChange={(e) => props.onChange(e.target.value)} spellCheck={false} />
        <div className="md overflow-auto rounded-md border border-line bg-bg p-3" dangerouslySetInnerHTML={{ __html: html }} />
      </div>
    </div>
  );
}
