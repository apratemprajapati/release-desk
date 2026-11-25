import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { Account, api, errText } from "../lib/api";
import { Button, Card, Field, inputCls } from "../components/ui";

export default function Settings() {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [label, setLabel] = useState("");
  const [token, setToken] = useState("");
  const [gitPath, setGitPath] = useState("");
  const [tagFmt, setTagFmt] = useState("v{version}");
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);

  const load = () => {
    api.accounts().then(setAccounts);
    api.settings().then((s) => { setGitPath(s.git_path); setTagFmt(s.default_tag_format); });
  };
  useEffect(load, []);

  async function addAccount() {
    setBusy(true); setErr(""); setMsg("");
    try {
      const a = await api.addAccount(label, token);
      setMsg(`Added ${a.label} (@${a.github_login}). Token stored in the OS keychain.`);
      setToken(""); setLabel(""); load();
    } catch (e) { setErr(errText(e)); } finally { setBusy(false); }
  }

  async function save() {
    setErr("");
    try {
      await api.setSetting("git_path", gitPath.trim());
      await api.setSetting("default_tag_format", tagFmt.trim());
      setMsg("Saved.");
    } catch (e) { setErr(errText(e)); }
  }

  return (
    <div className="max-w-3xl space-y-5 p-6">
      <h1 className="text-xl font-semibold">Settings</h1>
      {msg && <p className="text-ok">{msg}</p>}
      {err && <p className="text-bad">{err}</p>}

      <Card className="p-5">
        <h2 className="mb-1 font-semibold">GitHub account</h2>
        <p className="mb-4 text-xs text-mute">
          Use a fine-grained personal access token limited to the repos you release. Permissions: Contents read/write,
          Metadata read. The token is checked against GitHub, then kept only in the Windows Credential Manager.
        </p>
        {accounts.map((a) => (
          <div key={a.id} className="mb-2 flex items-center justify-between rounded-md border border-line bg-bg px-3 py-2">
            <span>{a.label} <span className="text-mute">@{a.github_login}</span></span>
            <Button variant="danger" onClick={async () => {
              if (await ask(`Remove ${a.label}? The token is deleted from the keychain.`, { kind: "warning" })) { await api.removeAccount(a.id); load(); }
            }}>Remove</Button>
          </div>
        ))}
        <div className="mt-3 grid grid-cols-[160px_1fr_auto] items-end gap-2">
          <Field label="Label"><input className={inputCls} value={label} onChange={(e) => setLabel(e.target.value)} placeholder="personal" /></Field>
          <Field label="Token">
            <input className={`${inputCls} font-mono`} type="password" autoComplete="off" value={token}
              onChange={(e) => setToken(e.target.value)} placeholder="github_pat_…" />
          </Field>
          <Button variant="primary" disabled={busy || !token.trim()} onClick={addAccount}>{busy ? "Checking…" : "Add"}</Button>
        </div>
        <p className="mt-2 text-[11px] text-mute">Phase 1 uses one account: the first one is the default for every project.</p>
      </Card>

      <Card className="space-y-4 p-5">
        <h2 className="font-semibold">General</h2>
        <Field label="Git executable" hint="Leave blank to use git from PATH.">
          <input className={inputCls} value={gitPath} onChange={(e) => setGitPath(e.target.value)} placeholder="C:\Program Files\Git\cmd\git.exe" />
        </Field>
        <Field label="Default tag format for new projects">
          <input className={inputCls} value={tagFmt} onChange={(e) => setTagFmt(e.target.value)} />
        </Field>
        <Button variant="primary" onClick={save} disabled={!tagFmt.includes("{version}")}>Save</Button>
      </Card>
    </div>
  );
}
