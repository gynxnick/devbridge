import { useEffect, useState } from "react";
import { Activity, FolderOpen, FolderPlus, GitBranch, GitMerge, Inbox, Link2, Monitor, Pause, TriangleAlert } from "lucide-react";
import { api } from "../lib/api";
import { useApp } from "../lib/app-context";
import { auditLabel, relativeTime, SYNC_STATE } from "../lib/format";
import type { AuditEvent, WorkspaceOffer, WorkspaceSummary } from "../lib/types";
import { Badge, Button, Card, CardHeader, Dot, EmptyState, ProjectGlyph } from "../components/ui";
import { ImportDialog, type ImportMode } from "../components/ImportDialog";
import { JoinOfferDialog } from "../components/JoinOfferDialog";
import { GettingStarted } from "../components/GettingStarted";

export function Dashboard() {
  const { overview, navigate, notify, refresh } = useApp();
  const [dialog, setDialog] = useState<ImportMode | null>(null);
  const [activity, setActivity] = useState<AuditEvent[]>([]);
  const [joining, setJoining] = useState<WorkspaceOffer | null>(null);

  useEffect(() => {
    api.listActivity(8).then(setActivity, () => setActivity([]));
  }, [overview]);

  return (
    <div className="space-y-6">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Dashboard</h1>
          <p className="text-sm text-muted">Your projects and the computers you work on them from.</p>
        </div>
        <div className="flex gap-2">
          <Button icon={FolderPlus} onClick={() => setDialog("create")}>
            Create project
          </Button>
          <Button icon={FolderOpen} variant="primary" onClick={() => setDialog("import")}>
            Import project
          </Button>
        </div>
      </header>

      <GettingStarted onAddProject={() => setDialog("import")} />

      {overview && overview.offers.length > 0 && (
        <Card>
          <CardHeader title="Shared with you" subtitle="Projects other devices offered to sync with this computer." />
          <ul className="divide-y divide-line">
            {overview.offers.map((o) => (
              <li key={o.workspaceId + o.deviceId} className="flex items-center gap-3 px-4 py-3">
                <Inbox size={18} className="text-info" />
                <div className="min-w-0 flex-1">
                  <p className="font-medium">{o.name}</p>
                  <p className="text-xs text-muted">
                    From {o.deviceName} · {relativeTime(o.offeredAt)}
                  </p>
                </div>
                <Button
                  variant="ghost"
                  onClick={async () => {
                    await api.declineOffer(o.workspaceId, o.deviceId).catch((e) => notify("danger", String(e)));
                    await refresh();
                  }}
                >
                  Dismiss
                </Button>
                <Button variant="primary" onClick={() => setJoining(o)}>
                  Join…
                </Button>
              </li>
            ))}
          </ul>
        </Card>
      )}

      <div className="grid gap-6 lg:grid-cols-[1fr_300px]">
        <Card>
          <CardHeader
            title="Projects"
            subtitle={overview ? `${overview.workspaces.length} registered on this device` : undefined}
          />
          {overview && overview.workspaces.length === 0 ? (
            <EmptyState icon={FolderOpen} title="No projects yet">
              Import an existing project folder or create a new one. DevBridge never moves or modifies your files when you add
              them.
            </EmptyState>
          ) : (
            <ul className="divide-y divide-line">
              {overview?.workspaces.map((w) => (
                <ProjectRow key={w.id} w={w} onOpen={() => navigate({ view: "project", id: w.id })} />
              ))}
            </ul>
          )}
        </Card>

        <div className="space-y-6">
          <Card>
            <CardHeader
              title="Devices"
              action={
                <Button variant="ghost" icon={Link2} onClick={() => navigate({ view: "devices" })}>
                  Connect device
                </Button>
              }
            />
            <div className="space-y-3 px-4 py-3 text-sm">
              {overview && (
                <div className="flex items-center gap-2">
                  <Monitor size={16} className="text-muted" />
                  <span className="flex-1 truncate font-medium">{overview.device.name}</span>
                  <Badge tone="ok">
                    <Dot tone="ok" />
                    This device
                  </Badge>
                </div>
              )}
              <p className="text-xs text-muted">
                {overview?.pairedDeviceCount
                  ? `${overview.onlineDeviceCount} of ${overview.pairedDeviceCount} paired device(s) online.`
                  : "No paired devices yet. Pair your other computer to start syncing."}
              </p>
            </div>
          </Card>

          <Card>
            <CardHeader title="Recent activity" />
            {activity.length === 0 ? (
              <EmptyState icon={Activity} title="Nothing yet" />
            ) : (
              <ul className="space-y-2 px-4 py-3 text-sm">
                {activity.map((a) => (
                  <li key={a.id} className="flex justify-between gap-2">
                    <span className="truncate">
                      {auditLabel(a.kind)}
                      {typeof a.detail?.name === "string" ? `: ${a.detail.name}` : ""}
                    </span>
                    <span className="shrink-0 text-xs text-muted">{relativeTime(a.at)}</span>
                  </li>
                ))}
              </ul>
            )}
          </Card>
        </div>
      </div>

      {dialog && <ImportDialog mode={dialog} onClose={() => setDialog(null)} />}
      {joining && <JoinOfferDialog offer={joining} onClose={() => setJoining(null)} />}
    </div>
  );
}

function ProjectRow({ w, onOpen }: { w: WorkspaceSummary; onOpen: () => void }) {
  const st = SYNC_STATE[w.syncState];
  return (
    <li>
      <button type="button" onClick={onOpen} className="flex w-full items-center gap-3 px-4 py-3 text-left hover:bg-surface-2">
        <ProjectGlyph color={w.color} icon={w.icon} />
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium">{w.name}</p>
          <p className="truncate font-mono text-xs text-muted" title={w.rootPath}>
            {w.rootPath}
          </p>
        </div>
        {!w.rootAvailable && (
          <Badge tone="danger">
            <TriangleAlert size={12} />
            Folder missing
          </Badge>
        )}
        {w.git?.lastBranch && (
          <span className="hidden items-center gap-1 text-xs text-muted sm:flex">
            <GitBranch size={13} />
            {w.git.lastBranch}
          </span>
        )}
        {w.openConflicts > 0 && (
          <Badge tone="danger">
            <GitMerge size={12} />
            {w.openConflicts} conflict{w.openConflicts > 1 ? "s" : ""}
          </Badge>
        )}
        {w.paused && <Pause size={14} className="text-warn" aria-label="Paused" />}
        {w.syncedDevices > 0 && (
          <span className="hidden text-xs text-muted md:inline">
            {w.onlineDevices}/{w.syncedDevices} online
          </span>
        )}
        <Badge tone={st.tone} title={st.help}>
          <Dot tone={st.tone} />
          {st.label}
        </Badge>
      </button>
    </li>
  );
}
