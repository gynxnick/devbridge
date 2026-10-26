import { useEffect, useState, type ReactNode } from "react";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { CircleCheck, CircleQuestionMark, Download, FolderOpen, RefreshCw, ShieldCheck, TriangleAlert } from "lucide-react";
import { api, errorMessage } from "../lib/api";
import { useApp } from "../lib/app-context";
import { auditLabel, bytes } from "../lib/format";
import type {
  AppSettings,
  AuditEvent,
  Diagnostics,
  EngineStatus,
  SyncDefaults,
  ThemePreference,
  ToolReport,
  ToolStatus,
} from "../lib/types";
import { Badge, Button, Card, CardHeader, Field, Notice, inputClass } from "../components/ui";

export function Settings() {
  const { settings } = useApp();
  if (!settings) return null;
  // Re-mount the form whenever the saved settings change so the draft resets.
  return <SettingsForm key={JSON.stringify(settings)} settings={settings} />;
}

function SettingsForm({ settings }: { settings: AppSettings }) {
  const { setSettings, notify } = useApp();
  const [draft, setDraft] = useState<AppSettings>(settings);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const dirty = JSON.stringify(draft) !== JSON.stringify(settings);
  const set = <K extends keyof AppSettings>(k: K, v: AppSettings[K]) => setDraft({ ...draft, [k]: v });

  async function save() {
    setBusy(true);
    setError(null);
    try {
      const saved = await api.saveSettings(draft);
      setSettings(saved);
      notify("ok", "Settings saved.");
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="space-y-6 pb-16">
      <header className="flex items-center justify-between">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Settings</h1>
          <p className="text-sm text-muted">Preferences for this device. They aren't synced to other computers.</p>
        </div>
        <div className="flex gap-2">
          <Button disabled={!dirty} onClick={() => setDraft(settings)}>
            Discard
          </Button>
          <Button variant="primary" disabled={!dirty} busy={busy} onClick={save}>
            Save changes
          </Button>
        </div>
      </header>
      {error && <Notice tone="danger">{error}</Notice>}

      <Card>
        <CardHeader title="Appearance" />
        <div className="px-4 py-3">
          <div role="radiogroup" aria-label="Theme" className="inline-flex rounded-md border border-line p-0.5">
            {(["system", "light", "dark"] as ThemePreference[]).map((t) => (
              <button
                key={t}
                type="button"
                role="radio"
                aria-checked={draft.theme === t}
                onClick={() => set("theme", t)}
                className={`rounded px-3 py-1 text-sm capitalize ${draft.theme === t ? "bg-surface-2 font-medium" : "text-muted"}`}
              >
                {t}
              </button>
            ))}
          </div>
        </div>
      </Card>

      <NetworkCard draft={draft} set={set} />
      <ToolsCard draft={draft} set={set} />
      <SyncRulesCard draft={draft} set={set} />

      <Card>
        <CardHeader title="Transfers & recovery" />
        <div className="grid gap-4 px-4 py-3 sm:grid-cols-3">
          <NumberField
            label="Concurrent transfers"
            value={draft.maxConcurrentTransfers}
            min={1}
            max={32}
            onChange={(v) => set("maxConcurrentTransfers", v)}
          />
          <NumberField
            label="Max file size (MB)"
            hint="Larger files aren't transferred."
            value={draft.maxFileSizeMb}
            min={1}
            max={1_048_576}
            onChange={(v) => set("maxFileSizeMb", v)}
          />
          <NumberField
            label="Full reconcile every (min)"
            value={draft.reconcileIntervalMinutes}
            min={1}
            max={1440}
            onChange={(v) => set("reconcileIntervalMinutes", v)}
          />
          <NumberField
            label="Keep overwrite history (days)"
            hint="Applies to copies of files replaced or deleted by sync. Conflict copies are kept until you delete them."
            value={draft.recoveryRetentionDays}
            min={1}
            max={3650}
            onChange={(v) => set("recoveryRetentionDays", v)}
          />
        </div>
      </Card>

      <Card>
        <CardHeader title="Startup & notifications" />
        <div className="space-y-2 px-4 py-3 text-sm">
          <Toggle
            label="Launch DevBridge when I log in"
            checked={draft.launchAtLogin}
            onChange={(v) => set("launchAtLogin", v)}
          />
          <Toggle label="Start minimized" checked={draft.startMinimized} onChange={(v) => set("startMinimized", v)} />
          <Toggle
            label="Notify me about conflicts, pairing requests, shared projects and sync errors when DevBridge isn't in front"
            checked={draft.notifications}
            onChange={(v) => set("notifications", v)}
          />
        </div>
      </Card>

      <DiagnosticsCard />
    </div>
  );
}

type Setter = <K extends keyof AppSettings>(k: K, v: AppSettings[K]) => void;

function Toggle({
  label,
  checked,
  onChange,
  extra,
}: {
  label: string;
  checked: boolean;
  onChange: (v: boolean) => void;
  extra?: ReactNode;
}) {
  return (
    <label className="flex items-center gap-2">
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span>{label}</span>
      {extra}
    </label>
  );
}

function NumberField({
  label,
  hint,
  value,
  min,
  max,
  onChange,
}: {
  label: string;
  hint?: string;
  value: number;
  min: number;
  max: number;
  onChange: (v: number) => void;
}) {
  return (
    <Field label={label} hint={hint}>
      <input
        type="number"
        className={inputClass}
        value={value}
        min={min}
        max={max}
        onChange={(e) => onChange(Number.isFinite(e.target.valueAsNumber) ? Math.trunc(e.target.valueAsNumber) : min)}
      />
    </Field>
  );
}

function NetworkCard({ draft, set }: { draft: AppSettings; set: Setter }) {
  const [status, setStatus] = useState<EngineStatus | null>(null);
  useEffect(() => {
    api.engineStatus().then(setStatus, () => {});
  }, []);
  return (
    <Card>
      <CardHeader title="Network" subtitle="How your other computers reach this one." />
      <div className="space-y-3 px-4 py-3 text-sm">
        <Toggle
          label="Allow connections from paired devices on this network"
          checked={draft.lanEnabled}
          onChange={(v) => set("lanEnabled", v)}
        />
        <p className="text-xs text-muted">
          Off: DevBridge listens only on this computer. On: it listens on your local network; every connection is encrypted (TLS
          1.3) and only devices you paired are accepted. Your firewall may ask to allow DevBridge the first time. DevBridge never
          uses a public relay.
        </p>
        <div className="grid gap-4 sm:grid-cols-3">
          <NumberField label="Port" value={draft.listenPort} min={1024} max={65535} onChange={(v) => set("listenPort", v)} />
        </div>
        {status && (
          <p className="text-xs text-muted">
            Currently listening on <span className="font-mono">{status.listenAddress ?? "nothing (see errors)"}</span>. Device
            identity <span className="font-mono">{status.shortFingerprint}</span>.
          </p>
        )}
      </div>
    </Card>
  );
}

function ToolsCard({ draft, set }: { draft: AppSettings; set: Setter }) {
  const { settings } = useApp();
  const [report, setReport] = useState<ToolReport | null>(null);
  const [busy, setBusy] = useState(false);
  const detect = async () => {
    setBusy(true);
    try {
      setReport(await api.detectTools());
    } finally {
      setBusy(false);
    }
  };
  const vscodePath = settings?.vscodePath;
  const gitPath = settings?.gitPath;
  useEffect(() => {
    let live = true;
    api.detectTools().then(
      (r) => live && setReport(r),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [vscodePath, gitPath]);

  return (
    <Card>
      <CardHeader
        title="Tools"
        subtitle="DevBridge opens projects with your own installations."
        action={
          <Button variant="ghost" icon={RefreshCw} busy={busy} onClick={detect}>
            Re-detect
          </Button>
        }
      />
      <div className="space-y-4 px-4 py-3">
        {report && (
          <ul className="space-y-2">
            {[report.vscode, report.git, report.claudeExtension, report.claudeCli].map((t) => (
              <ToolRow key={t.name} t={t} />
            ))}
          </ul>
        )}
        <div className="grid gap-4 sm:grid-cols-2">
          <Field
            label="VS Code command path"
            hint="Leave empty to auto-detect. macOS: …/Visual Studio Code.app/Contents/Resources/app/bin/code. Windows: …\Microsoft VS Code\bin\code.cmd"
          >
            <input
              className={`${inputClass} font-mono`}
              placeholder="Auto-detect"
              value={draft.vscodePath ?? ""}
              onChange={(e) => set("vscodePath", e.target.value || null)}
            />
          </Field>
          <Field label="Git executable path" hint="Leave empty to auto-detect.">
            <input
              className={`${inputClass} font-mono`}
              placeholder="Auto-detect"
              value={draft.gitPath ?? ""}
              onChange={(e) => set("gitPath", e.target.value || null)}
            />
          </Field>
        </div>
        <p className="text-xs text-muted">
          DevBridge never reads, copies or syncs Claude Code credentials, settings or conversations. Sign in to Claude Code on
          each computer as usual. Syncing files doesn't move a running Claude Code session or terminal; start a new one on the
          other computer.
        </p>
      </div>
    </Card>
  );
}

function ToolRow({ t }: { t: ToolStatus }) {
  const optional = t.name === "Claude Code CLI";
  return (
    <li className="flex items-start gap-3 text-sm">
      {t.found && t.version ? (
        <CircleCheck size={16} className="mt-0.5 text-ok" />
      ) : t.found || optional ? (
        <CircleQuestionMark size={16} className="mt-0.5 text-warn" />
      ) : (
        <TriangleAlert size={16} className="mt-0.5 text-danger" />
      )}
      <div className="min-w-0 flex-1">
        <p className="font-medium">
          {t.name} {t.version && <span className="font-normal text-muted">{t.version}</span>}
          {t.source === "custom" && (
            <span className="ml-1">
              <Badge tone="info">custom path</Badge>
            </span>
          )}
        </p>
        {t.path && (
          <p className="truncate font-mono text-xs text-muted" title={t.path}>
            {t.path}
          </p>
        )}
        {t.hint && <p className="text-xs text-muted">{t.hint}</p>}
      </div>
    </li>
  );
}

function SyncRulesCard({ draft, set }: { draft: AppSettings; set: Setter }) {
  const [defaults, setDefaults] = useState<SyncDefaults | null>(null);
  useEffect(() => {
    api.syncDefaults().then(setDefaults, () => setDefaults(null));
  }, []);

  const toggleSecret = (p: string, allow: boolean) =>
    set("secretOptins", allow ? [...draft.secretOptins, p] : draft.secretOptins.filter((x) => x !== p));

  return (
    <Card>
      <CardHeader title="Sync rules" subtitle="What is never, or not by default, copied between devices." />
      <div className="space-y-4 px-4 py-3 text-sm">
        {defaults && (
          <>
            <p>
              <span className="font-medium">Always excluded: </span>
              {defaults.alwaysExcluded.map((p) => (
                <code key={p} className="mr-1 rounded bg-surface-2 px-1 font-mono text-xs">
                  {p}
                </code>
              ))}
              <span className="text-xs text-muted">Git history moves through your Git remotes instead.</span>
            </p>
            <details>
              <summary className="cursor-pointer font-medium">Default exclusions ({defaults.defaultExcludes.length})</summary>
              <p className="mt-2 font-mono text-xs leading-6 text-muted">{defaults.defaultExcludes.join("  ")}</p>
              <p className="text-xs text-muted">
                Override one with a <code className="font-mono">!pattern</code> line below.
              </p>
            </details>
            <fieldset className="space-y-1">
              <legend className="flex items-center gap-1.5 font-medium">
                <ShieldCheck size={15} className="text-ok" />
                Secret files (not synced unless you allow them)
              </legend>
              <div className="grid gap-x-4 gap-y-1 sm:grid-cols-3">
                {defaults.secretPatterns.map((p) => (
                  <label key={p} className="flex items-center gap-2 font-mono text-xs">
                    <input
                      type="checkbox"
                      checked={draft.secretOptins.includes(p)}
                      onChange={(e) => toggleSecret(p, e.target.checked)}
                    />
                    <span>{draft.secretOptins.includes(p) ? <span className="text-warn">sync {p}</span> : p}</span>
                  </label>
                ))}
              </div>
              {draft.secretOptins.length > 0 && (
                <Notice tone="warn">
                  Checked patterns will be copied to every paired device. Only allow files you'd be comfortable having on all of
                  them. Changes are recorded in the activity log.
                </Notice>
              )}
            </fieldset>
          </>
        )}
        <Field label="Your exclusion rules" hint="One pattern per line, gitignore syntax. Applies to all projects.">
          <textarea
            rows={4}
            className={`${inputClass} font-mono`}
            value={draft.excludePatterns.join("\n")}
            onChange={(e) => set("excludePatterns", e.target.value.split("\n"))}
          />
        </Field>
        <Toggle
          label="Also respect each project's .gitignore"
          checked={draft.respectGitignore}
          onChange={(v) => set("respectGitignore", v)}
        />
        <p className="text-xs text-muted">Excluding a file later doesn't delete it on other devices; it simply stops syncing.</p>
      </div>
    </Card>
  );
}

function DiagnosticsCard() {
  const { notify } = useApp();
  const [diag, setDiag] = useState<Diagnostics | null>(null);
  const [log, setLog] = useState<AuditEvent[]>([]);
  useEffect(() => {
    api.diagnostics().then(setDiag, () => setDiag(null));
    api.listActivity(30).then(setLog, () => setLog([]));
  }, []);

  const exportReport = async () => {
    const path = await saveDialog({
      title: "Save diagnostic report",
      defaultPath: `devbridge-diagnostics-${new Date().toISOString().slice(0, 10)}.json`,
      filters: [{ name: "JSON", extensions: ["json"] }],
    });
    if (!path) return;
    try {
      await api.exportDiagnostics(path);
      notify("ok", "Diagnostic report saved.");
    } catch (e) {
      notify("danger", errorMessage(e));
    }
  };

  return (
    <Card>
      <CardHeader
        title="Logs & diagnostics"
        subtitle="Reports contain versions, settings, counts, project paths and recent log lines — never file contents, keys or tokens."
      />
      <div className="space-y-3 px-4 py-3 text-sm">
        {diag && (
          <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1">
            <dt className="text-muted">Version</dt>
            <dd>{diag.appVersion}</dd>
            <dt className="text-muted">System</dt>
            <dd>
              {diag.os} ({diag.arch})
            </dd>
            <dt className="text-muted">Database schema</dt>
            <dd>v{diag.schemaVersion}</dd>
            <dt className="text-muted">Device key</dt>
            <dd className={diag.identityStorage?.startsWith("temporary") ? "text-warn" : ""}>{diag.identityStorage}</dd>
            <dt className="text-muted">Recovery copies</dt>
            <dd>{bytes(diag.recoveryBytes)}</dd>
            <dt className="text-muted">Data folder</dt>
            <dd className="truncate font-mono text-xs" title={diag.dataDir}>
              {diag.dataDir}
            </dd>
          </dl>
        )}
        <div className="flex flex-wrap gap-2">
          <Button icon={Download} onClick={exportReport}>
            Export diagnostic report…
          </Button>
          {diag && (
            <Button icon={FolderOpen} onClick={() => revealItemInDir(diag.logsDir).catch((e) => notify("danger", String(e)))}>
              Show log files
            </Button>
          )}
        </div>
        <details>
          <summary className="cursor-pointer font-medium">Activity log</summary>
          <ul className="mt-2 space-y-1 font-mono text-xs text-muted">
            {log.map((e) => (
              <li key={e.id}>
                {new Date(e.at).toLocaleString()} · {auditLabel(e.kind)}
              </li>
            ))}
          </ul>
        </details>
      </div>
    </Card>
  );
}
