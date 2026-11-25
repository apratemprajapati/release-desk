import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { Account, api, DeviceCode, errText, openUrl } from "../lib/api";
import { Button, Card, Field, inputCls } from "../components/ui";
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";

export default function Settings() {
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [label, setLabel] = useState("");
  const [token, setToken] = useState("");
  const [gitPath, setGitPath] = useState("");
  const [tagFmt, setTagFmt] = useState("v{version}");
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const [clientId, setClientId] = useState("");
  const [device, setDevice] = useState<DeviceCode | null>(null);
  const [aiKey, setAiKey] = useState("");
  const [aiModel, setAiModel] = useState("");
  const [aiSet, setAiSet] = useState(false);
  const [appVersion, setAppVersion] = useState("");
  const [update, setUpdate] = useState("");

  const load = () => {
    api.accounts().then(setAccounts);
    api.settings().then((s) => { setGitPath(s.git_path); setTagFmt(s.default_tag_format); setClientId(s.oauth_client_id); setAiModel(s.ai_model); setAiSet(s.ai_key_set); });
    getVersion().then(setAppVersion).catch(() => {});
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

  async function signIn() {
    setErr(""); setMsg("");
    try {
      await api.setSetting("oauth_client_id", clientId.trim());
      const d = await api.deviceStart();
      setDevice(d);
      await openUrl(d.verification_uri);
      let interval = d.interval;
      const deadline = Date.now() + d.expires_in * 1000;
      while (Date.now() < deadline) {
        await new Promise((r) => setTimeout(r, interval * 1000));
        const r = await api.devicePoll(d.device_code, interval, label);
        if (r.state === "done") {
          setMsg(`Signed in as @${r.account!.github_login}. Token stored in the OS keychain.`);
          setDevice(null); setLabel(""); load();
          return;
        }
        interval = r.interval;
      }
      setErr("The code expired. Start again.");
    } catch (e) {
      setErr(errText(e));
    }
    setDevice(null);
  }

  async function saveAi(clear = false) {
    setErr("");
    try {
      await api.setAiKey(clear ? "" : aiKey);
      await api.setSetting("ai_model", aiModel.trim());
      setAiKey("");
      setMsg(clear ? "AI key removed." : "AI settings saved. The key is in the OS keychain.");
      load();
    } catch (e) { setErr(errText(e)); }
  }

  async function checkUpdate() {
    setUpdate("Checking…");
    try {
      const u = await check();
      if (!u) return setUpdate("You have the latest version.");
      setUpdate(`Downloading ${u.version}…`);
      await u.downloadAndInstall();
      setUpdate(`Installed ${u.version}. Restarting…`);
      await relaunch();
    } catch (e) {
      setUpdate(`Update check failed: ${errText(e)}`);
    }
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
          Use a fine-grained personal access token limited to the repos you release. Permissions: Contents, Pull requests
          and Actions read/write (Metadata read is automatic). The token is checked against GitHub, then kept only in the Windows Credential Manager.
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
        <p className="mt-2 text-[11px] text-mute">Each project chooses its account under Edit project; projects without one use the first account.</p>

        <div className="mt-5 border-t border-line pt-4">
          <h3 className="mb-1 text-[13px] font-semibold">Or sign in with GitHub (device flow)</h3>
          <p className="mb-3 text-xs text-mute">
            Needs the client ID of a GitHub OAuth App you register once (Settings → Developer settings → OAuth Apps, with
            "Enable Device Flow" ticked). The token gets the repo and workflow scopes.
          </p>
          <div className="grid grid-cols-[1fr_auto] items-end gap-2">
            <Field label="OAuth App client ID"><input className={`${inputCls} font-mono`} value={clientId} onChange={(e) => setClientId(e.target.value)} placeholder="Ov23li…" /></Field>
            <Button variant="primary" disabled={!clientId.trim() || !!device} onClick={signIn}>Sign in with GitHub</Button>
          </div>
          {device && (
            <div className="mt-3 rounded-md border border-accent/40 bg-accent/10 p-3 text-[13px]">
              Enter this code at <button className="text-accent underline" onClick={() => openUrl(device.verification_uri)}>{device.verification_uri}</button>:
              <div className="my-2 font-mono text-2xl tracking-widest">{device.user_code}</div>
              <span className="text-xs text-mute">Waiting for approval… (the label above is used for the account)</span>
            </div>
          )}
        </div>
      </Card>

      <Card className="p-5">
        <h2 className="mb-1 font-semibold">AI notes polishing (optional)</h2>
        <p className="mb-4 text-xs text-mute">
          Off unless you add your own Anthropic API key. Only the release notes text is sent, and only when you press
          "Polish with AI" and confirm. {aiSet ? "A key is stored in the OS keychain." : "No key stored."}
        </p>
        <div className="grid grid-cols-[1fr_200px_auto_auto] items-end gap-2">
          <Field label="API key"><input className={`${inputCls} font-mono`} type="password" autoComplete="off" value={aiKey}
            onChange={(e) => setAiKey(e.target.value)} placeholder={aiSet ? "stored; type to replace" : "sk-ant-…"} /></Field>
          <Field label="Model"><input className={`${inputCls} font-mono`} value={aiModel} onChange={(e) => setAiModel(e.target.value)} /></Field>
          <Button variant="primary" disabled={!aiKey.trim() && !aiSet} onClick={() => saveAi()}>Save</Button>
          <Button variant="danger" disabled={!aiSet} onClick={() => saveAi(true)}>Remove key</Button>
        </div>
      </Card>

      <Card className="p-5">
        <h2 className="mb-1 font-semibold">Updates</h2>
        <p className="mb-3 text-xs text-mute">Release Desk {appVersion}. Updates come from the signed releases of apratemprajapati/release-desk.</p>
        <div className="flex items-center gap-3">
          <Button onClick={checkUpdate}>Check for updates</Button>
          {update && <span className="text-xs text-mute">{update}</span>}
        </div>
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
