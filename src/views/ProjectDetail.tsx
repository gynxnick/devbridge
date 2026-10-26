import { useCallback, useEffect, useState } from "react";
import {
  CodeXml,
  Pause,
  Pencil,
  Play,
  RotateCcw,
  ScanSearch,
  Server,
  Share2,
  Terminal,
  Trash,
  TriangleAlert,
  Unlink,
} from "lucide-react";
import { api, errorMessage } from "../lib/api";
import { useApp, useEngineEvents } from "../lib/app-context";
import { bytes, relativeTime, SYNC_STATE } from "../lib/format";
import {
  WORKSPACE_COLORS,
  WORKSPACE_ICONS,
  type DeviceView,
  type RemoteStatus,
  type WorkspaceColor,
  type WorkspaceIcon,
  type WorkspaceSummary,
  type WorkspaceSync,
} from "../lib/types";
import {
  Badge,
  Button,
  Card,
  CardHeader,
  COLOR_HEX,
  Dot,
  Field,
  ICONS,
  Modal,
  Notice,
  ProjectGlyph,
  inputClass,
} from "../components/ui";
import { GitPanel } from "../components/GitPanel";

type Tab = "sync" | "git" | "remote";

export function ProjectDetail({ id }: { id: string }) {
  const { overview, notify } = useApp();
  const w = overview?.workspaces.find((x) => x.id === id);
  const [busy, setBusy] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [tab, setTab] = useState<Tab>("sync");

  if (!overview) return null;
  if (!w) return <Notice tone="warn">This project is no longer registered.</Notice>;

  async function run(key: string, fn: () => Promise<unknown>, ok?: string) {
    setBusy(key);
    try {
      await fn();
      if (ok) notify("ok", ok);
    } catch (e) {
      notify("danger", errorMessage(e));
    } finally {
      setBusy(null);
    }
  }

  return (
    <div className="space-y-6">
      <header className="flex flex-wrap items-start gap-4">
        <ProjectGlyph color={w.color} icon={w.icon} size={48} />
        <div className="min-w-0 flex-1">
          <h1 className="flex items-center gap-2 text-xl font-semibold tracking-tight">
            {w.name}
            <button
              type="button"
              aria-label="Edit name and appearance"
              onClick={() => setEditing(true)}
              className="rounded p-1 text-muted hover:bg-surface-2 hover:text-fg"
            >
              <Pencil size={15} />
            </button>
          </h1>
          <p className="truncate font-mono text-xs text-muted" title={w.rootPath}>
            {w.rootPath}
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Button
            variant="primary"
            icon={CodeXml}
            busy={busy === "code"}
            disabled={!w.rootAvailable}
            onClick={() => run("code", () => api.openInVscode(w.id))}
          >
            Open in VS Code
          </Button>
          <Button
            icon={Terminal}
            busy={busy === "term"}
            disabled={!w.rootAvailable}
            onClick={() => run("term", () => api.openTerminal(w.id))}
          >
            Open Terminal
          </Button>
        </div>
      </header>

      {!w.rootAvailable && (
        <Notice tone="danger">
          The project folder isn't available. If it's on an external disk, reconnect it. DevBridge won't sync or change anything
          until the folder is back.
        </Notice>
      )}

      <div role="tablist" aria-label="Project sections" className="flex gap-1 border-b border-line">
        {(
          [
            ["sync", "Sync"],
            ["git", "Git"],
            ["remote", "Remote access"],
          ] as [Tab, string][]
        ).map(([t, label]) => (
          <button
            key={t}
            role="tab"
            type="button"
            aria-selected={tab === t}
            onClick={() => setTab(t)}
            className={`-mb-px border-b-2 px-3 py-2 text-sm ${tab === t ? "border-accent font-medium" : "border-transparent text-muted hover:text-fg"}`}
          >
            {label}
          </button>
        ))}
      </div>

      {tab === "sync" && <SyncTab w={w} />}
      {tab === "git" && <GitPanel id={w.id} />}
      {tab === "remote" && <RemoteTab w={w} />}

      <Card>
        <CardHeader title="Danger zone" />
        <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3 text-sm">
          <p className="text-muted">
            Remove this project from DevBridge. Your files and Git repository stay exactly where they are.
          </p>
          <Button variant="secondary" icon={Trash} onClick={() => setRemoving(true)}>
            Remove from DevBridge
          </Button>
        </div>
      </Card>

      {/* Mounted only while open so each opening starts from the saved values. */}
      {editing && <EditDialog w={w} onClose={() => setEditing(false)} />}
      {removing && <RemoveDialog w={w} onClose={() => setRemoving(false)} />}
    </div>
  );
}

function Stat({ label, value, tone }: { label: string; value: string; tone?: "danger" }) {
  return (
    <div>
      <dt className="text-xs text-muted">{label}</dt>
      <dd className={`font-medium ${tone === "danger" ? "text-danger" : ""}`}>{value}</dd>
    </div>
  );
}

function SyncTab({ w }: { w: WorkspaceSummary }) {
  const { overview, notify, refresh, navigate } = useApp();
  const [sync, setSync] = useState<WorkspaceSync | null>(null);
  const [devices, setDevices] = useState<DeviceView[]>([]);
  const [progress, setProgress] = useState<Record<string, { done: number; total: number }>>({});
  const [unsharing, setUnsharing] = useState<{ id: string; name: string } | null>(null);

  const load = useCallback(async () => {
    try {
      const [s, d] = await Promise.all([api.workspaceSync(w.id), api.listDevices()]);
      setSync(s);
      setDevices(d.filter((x) => !x.isLocal && x.status === "paired"));
    } catch (e) {
      notify("danger", errorMessage(e));
    }
  }, [w.id, notify]);

  useEffect(() => {
    let live = true;
    Promise.all([api.workspaceSync(w.id), api.listDevices()]).then(
      ([s, d]) => {
        if (!live) return;
        setSync(s);
        setDevices(d.filter((x) => !x.isLocal && x.status === "paired"));
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [w.id]);

  useEngineEvents((e) => {
    if (e.kind === "transferProgress" && e.workspaceId === w.id) {
      setProgress((p) => ({ ...p, [e.path]: { done: e.done, total: e.total } }));
    } else if (
      (e.kind === "workspaceChanged" && e.workspaceId === w.id) ||
      e.kind === "devicesChanged" ||
      e.kind === "peerOnline" ||
      e.kind === "peerOffline"
    ) {
      void load();
    }
  });

  const st = SYNC_STATE[w.syncState];
  const act = async (fn: () => Promise<unknown>, ok?: string) => {
    try {
      await fn();
      if (ok) notify("ok", ok);
      await Promise.all([load(), refresh()]);
    } catch (e) {
      notify("danger", errorMessage(e));
    }
  };
  const shares = new Map(sync?.shares.map((s) => [s.deviceId, s]));

  return (
    <div className="grid gap-6 md:grid-cols-2">
      <Card>
        <CardHeader
          title="Synchronization"
          action={
            <Badge tone={st.tone}>
              <Dot tone={st.tone} />
              {st.label}
            </Badge>
          }
        />
        <dl className="grid grid-cols-2 gap-x-4 gap-y-3 px-4 py-3 text-sm">
          <Stat label="Mode" value={w.mode === "synced" ? "Synced project" : "Remote workspace"} />
          <Stat
            label="Last edited on"
            value={
              w.activeDeviceId === overview?.device.id
                ? `${overview?.device.name} (this device)`
                : (devices.find((d) => d.id === w.activeDeviceId)?.name ?? "Another device")
            }
          />
          <Stat label="Last successful sync" value={relativeTime(w.lastSyncedAt)} />
          <Stat label="Pending changes" value={String(w.pendingChanges)} />
          <div>
            <dt className="text-xs text-muted">Open conflicts</dt>
            <dd>
              {w.openConflicts > 0 ? (
                <button
                  type="button"
                  className="font-medium text-danger underline"
                  onClick={() => navigate({ view: "conflicts" })}
                >
                  {w.openConflicts} — review
                </button>
              ) : (
                <span className="font-medium">0</span>
              )}
            </dd>
          </div>
          <Stat label="Files that can't sync" value={String(w.openIssues)} />
        </dl>
        <p className="px-4 text-xs text-muted">{st.help}</p>
        <div className="flex flex-wrap gap-2 px-4 py-3">
          {w.paused ? (
            <Button icon={Play} onClick={() => act(() => api.setPaused(w.id, false), "Sync resumed.")}>
              Resume sync
            </Button>
          ) : (
            <Button
              icon={Pause}
              disabled={w.syncedDevices === 0}
              onClick={() => act(() => api.setPaused(w.id, true), "Sync paused.")}
            >
              Pause sync
            </Button>
          )}
          <Button icon={ScanSearch} onClick={() => act(() => api.rescan(w.id), "Reconciling with disk and devices…")}>
            Reconcile
          </Button>
          {(sync?.failedTransfers.length ?? 0) > 0 && (
            <Button icon={RotateCcw} onClick={() => act(() => api.retryTransfers(w.id), "Retrying failed transfers…")}>
              Retry failed
            </Button>
          )}
        </div>
      </Card>

      <Card>
        <CardHeader title="Devices" subtitle="Share this project with your paired computers." />
        {devices.length === 0 ? (
          <div className="px-4 py-3 text-sm text-muted">
            No paired devices.{" "}
            <button type="button" className="text-accent underline" onClick={() => navigate({ view: "devices" })}>
              Pair one
            </button>{" "}
            to sync this project.
          </div>
        ) : (
          <ul className="divide-y divide-line">
            {devices.map((d) => {
              const s = shares.get(d.id);
              return (
                <li key={d.id} className="flex items-center gap-3 px-4 py-2 text-sm">
                  <Dot tone={d.online ? "ok" : "muted"} />
                  <span className="flex-1 font-medium">{d.name}</span>
                  {s?.joined ? (
                    <Badge tone="ok">Syncing{s.lastSyncAt ? ` · ${relativeTime(s.lastSyncAt)}` : ""}</Badge>
                  ) : s?.shared ? (
                    <Badge tone="warn">Offered — waiting to join</Badge>
                  ) : null}
                  {s ? (
                    <Button variant="ghost" icon={Unlink} onClick={() => setUnsharing({ id: d.id, name: d.name })}>
                      Stop
                    </Button>
                  ) : (
                    <Button icon={Share2} onClick={() => act(() => api.shareWorkspace(w.id, d.id), `Offered to ${d.name}.`)}>
                      Share
                    </Button>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </Card>

      <Card className="md:col-span-2">
        <CardHeader
          title="Transfers"
          subtitle={sync ? `${sync.queuedChanges} change notification(s) waiting to be acknowledged` : undefined}
        />
        {!sync || (sync.transfers.length === 0 && sync.failedTransfers.length === 0) ? (
          <p className="px-4 py-3 text-sm text-muted">No transfers in progress.</p>
        ) : (
          <ul className="divide-y divide-line">
            {sync.transfers.map((t) => {
              const p = progress[t.path];
              const done = p?.done ?? t.bytesDone;
              const pct = t.totalSize ? Math.min(100, Math.round((done / t.totalSize) * 100)) : 0;
              return (
                <li key={t.id} className="px-4 py-2 text-sm">
                  <div className="flex justify-between gap-2">
                    <span className="truncate font-mono text-xs">{t.path}</span>
                    <span className="shrink-0 text-xs text-muted">
                      {bytes(done)} / {bytes(t.totalSize)}
                    </span>
                  </div>
                  <div
                    className="mt-1 h-1.5 overflow-hidden rounded bg-surface-2"
                    role="progressbar"
                    aria-valuenow={pct}
                    aria-valuemin={0}
                    aria-valuemax={100}
                  >
                    <div className="h-full bg-info" style={{ width: `${pct}%` }} />
                  </div>
                </li>
              );
            })}
            {sync.failedTransfers.map((t) => (
              <li key={t.id} className="flex gap-2 px-4 py-2 text-sm">
                <TriangleAlert size={15} className="mt-0.5 shrink-0 text-danger" />
                <span className="truncate font-mono text-xs">{t.path}</span>
                <span className="text-xs text-danger">{t.lastError}</span>
              </li>
            ))}
          </ul>
        )}
      </Card>

      {unsharing && (
        <Modal
          open
          title={`Stop syncing with ${unsharing.name}?`}
          onClose={() => setUnsharing(null)}
          footer={
            <>
              <Button onClick={() => setUnsharing(null)}>Cancel</Button>
              <Button
                variant="danger"
                onClick={() => {
                  const u = unsharing;
                  setUnsharing(null);
                  void act(() => api.unshareWorkspace(w.id, u.id), `No longer syncing with ${u.name}.`);
                }}
              >
                Stop syncing
              </Button>
            </>
          }
        >
          <p className="text-sm">
            Both computers keep their current files. Future changes won't be exchanged until you share the project again.
          </p>
        </Modal>
      )}
    </div>
  );
}

function RemoteTab({ w }: { w: WorkspaceSummary }) {
  const { notify } = useApp();
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  useEffect(() => {
    let live = true;
    api.remoteStatus().then(
      (s) => live && setStatus(s),
      () => {},
    );
    return () => {
      live = false;
    };
  }, []);
  if (!status) return null;
  const hosting = status.hosting.includes(w.id);
  const toggle = async () => {
    try {
      await api.remoteSetHosting(w.id, !hosting);
      setStatus(await api.remoteStatus());
      notify("ok", hosting ? "Remote access turned off for this project." : "Remote access turned on for this project.");
    } catch (e) {
      notify("danger", errorMessage(e));
    }
  };
  return (
    <Card>
      <CardHeader
        title="Remote Workspace"
        subtitle="Work on this computer's copy from your other computer, through VS Code Remote-SSH."
        action={<Badge tone={hosting ? "ok" : "muted"}>{hosting ? "Available remotely" : "Not shared remotely"}</Badge>}
      />
      <div className="space-y-3 px-4 py-3 text-sm">
        <p>
          With Remote Workspace, files, terminals, builds and Claude Code all run on <strong>this</strong> computer; the other
          computer only shows VS Code. There's one authoritative copy, so nothing can conflict.
        </p>
        {!status.sshServerRunning && (
          <Notice tone="warn">
            <p className="font-medium">This computer isn't accepting SSH connections yet.</p>
            <p className="mt-1">{status.sshServerGuidance}</p>
          </Notice>
        )}
        {w.syncedDevices > 0 && hosting && (
          <Notice tone="info">
            This project also syncs to another device. While working on it remotely, avoid editing that device's synced copy at
            the same time (or pause sync there) so two Claude Code sessions don't edit the same files independently.
          </Notice>
        )}
        <div className="flex gap-2">
          <Button variant={hosting ? "secondary" : "primary"} icon={Server} onClick={toggle}>
            {hosting ? "Turn off remote access" : "Allow remote access to this project"}
          </Button>
        </div>
        <p className="text-xs text-muted">
          Paired devices must request access (Devices → Remote access) and you approve each one. Their key can be revoked at any
          time.
        </p>
      </div>
    </Card>
  );
}

function EditDialog({ w, onClose }: { w: WorkspaceSummary; onClose: () => void }) {
  const { refresh, notify } = useApp();
  const [name, setName] = useState(w.name);
  const [color, setColor] = useState<WorkspaceColor>(w.color);
  const [icon, setIcon] = useState<WorkspaceIcon>(w.icon);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function save() {
    setBusy(true);
    try {
      await api.updateWorkspace(w.id, name, color, icon);
      await refresh();
      notify("ok", "Project updated.");
      onClose();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Modal
      open
      onClose={onClose}
      title="Edit project"
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" busy={busy} onClick={save} disabled={!name.trim()}>
            Save
          </Button>
        </>
      }
    >
      {error && <Notice tone="danger">{error}</Notice>}
      <Field label="Name">
        <input className={inputClass} value={name} maxLength={80} onChange={(e) => setName(e.target.value)} />
      </Field>
      <div className="flex flex-wrap gap-1.5" role="group" aria-label="Color">
        {WORKSPACE_COLORS.map((c) => (
          <button
            key={c}
            type="button"
            aria-label={c}
            aria-pressed={c === color}
            onClick={() => setColor(c)}
            className={`h-6 w-6 rounded-full ring-offset-2 ring-offset-surface ${c === color ? "ring-2 ring-fg" : ""}`}
            style={{ background: COLOR_HEX[c] }}
          />
        ))}
      </div>
      <div className="flex flex-wrap gap-1.5" role="group" aria-label="Icon">
        {WORKSPACE_ICONS.map((i) => {
          const Icon = ICONS[i];
          return (
            <button
              key={i}
              type="button"
              aria-label={i}
              aria-pressed={i === icon}
              onClick={() => setIcon(i)}
              className={`rounded-md border p-1.5 ${i === icon ? "border-accent text-accent" : "border-line text-muted hover:text-fg"}`}
            >
              <Icon size={16} />
            </button>
          );
        })}
      </div>
    </Modal>
  );
}

function RemoveDialog({ w, onClose }: { w: WorkspaceSummary; onClose: () => void }) {
  const { refresh, notify, navigate } = useApp();
  const [busy, setBusy] = useState(false);
  async function remove() {
    setBusy(true);
    try {
      await api.removeWorkspace(w.id);
      navigate({ view: "dashboard" });
      await refresh();
      notify("ok", `Removed “${w.name}” from DevBridge. Files were not touched.`);
    } catch (e) {
      notify("danger", errorMessage(e));
    } finally {
      setBusy(false);
      onClose();
    }
  }
  return (
    <Modal
      open
      onClose={onClose}
      title={`Remove “${w.name}”?`}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="danger" busy={busy} onClick={remove}>
            Remove from DevBridge
          </Button>
        </>
      }
    >
      <p className="text-sm">
        DevBridge will forget this project and its sync history on this device, and stop syncing it with other devices.
      </p>
      <Notice tone="info">
        Nothing in <span className="font-mono">{w.rootPath}</span> is deleted or changed, including the{" "}
        <span className="font-mono">.git</span> folder. Recovery copies are kept until you delete them in the Conflicts → Recovery
        tab.
      </Notice>
    </Modal>
  );
}
