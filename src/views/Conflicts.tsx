import { useCallback, useEffect, useMemo, useState } from "react";
import { CircleCheck, FileWarning, GitMerge, History, RotateCcw, Search, Trash } from "lucide-react";
import { api, errorMessage } from "../lib/api";
import { useApp, useEngineEvents } from "../lib/app-context";
import { bytes, CONFLICT_KIND, ISSUE_LABEL, RECOVERY_REASON, RESOLUTION_LABEL, relativeTime } from "../lib/format";
import { hasMarkers, sideBySide, withMarkers } from "../lib/merge";
import type { ConflictDetail, ConflictView, RecoveryItem, SyncIssue } from "../lib/types";
import { Badge, Button, Card, CardHeader, EmptyState, Modal, Notice, inputClass } from "../components/ui";

type Tab = "conflicts" | "issues" | "recovery";

export function Conflicts() {
  const { overview, route } = useApp();
  const [tab, setTab] = useState<Tab>("conflicts");
  const total = overview?.workspaces.reduce((n, w) => n + w.openConflicts, 0) ?? 0;
  const issues = overview?.workspaces.reduce((n, w) => n + w.openIssues, 0) ?? 0;
  return (
    <div className="space-y-6">
      <header>
        <h1 className="text-xl font-semibold tracking-tight">Conflicts</h1>
        <p className="text-sm text-muted">
          Files changed on two devices before they could sync. Both versions are always kept until you decide.
        </p>
      </header>
      <div role="tablist" className="flex gap-1 border-b border-line">
        {(
          [
            ["conflicts", `Conflicts (${total})`],
            ["issues", `Can't sync (${issues})`],
            ["recovery", "Recovery history"],
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
      {tab === "conflicts" && <ConflictList initial={route.view === "conflicts" ? route.conflictId : undefined} />}
      {tab === "issues" && <Issues />}
      {tab === "recovery" && <Recovery />}
    </div>
  );
}

function ConflictList({ initial }: { initial?: string }) {
  const { overview, notify } = useApp();
  const [items, setItems] = useState<ConflictView[]>([]);
  const [query, setQuery] = useState("");
  const [project, setProject] = useState("");
  const [showResolved, setShowResolved] = useState(false);
  const [selected, setSelected] = useState<string | null>(initial ?? null);

  const load = useCallback(
    () => api.listConflicts(null, showResolved).then(setItems, (e) => notify("danger", errorMessage(e))),
    [showResolved, notify],
  );
  useEffect(() => {
    void load();
  }, [load]);
  useEngineEvents((e) => {
    if (e.kind === "conflictDetected" || e.kind === "workspaceChanged") void load();
  });

  const filtered = items.filter(
    (c) => (!project || c.workspaceId === project) && (!query || c.path.toLowerCase().includes(query.toLowerCase())),
  );
  const current = selected ?? filtered.find((c) => c.state === "open")?.id ?? null;

  return (
    <div className="grid gap-6 lg:grid-cols-[320px_1fr]">
      <Card>
        <div className="space-y-2 border-b border-line p-3">
          <label className="relative block">
            <Search size={14} className="absolute top-2.5 left-2 text-muted" />
            <input
              aria-label="Search conflicts"
              placeholder="Search by file"
              className={`${inputClass} pl-7`}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
          </label>
          <div className="flex items-center gap-2">
            <select
              aria-label="Filter by project"
              className={`${inputClass} !w-auto flex-1`}
              value={project}
              onChange={(e) => setProject(e.target.value)}
            >
              <option value="">All projects</option>
              {overview?.workspaces.map((w) => (
                <option key={w.id} value={w.id}>
                  {w.name}
                </option>
              ))}
            </select>
            <label className="flex items-center gap-1 text-xs whitespace-nowrap">
              <input type="checkbox" checked={showResolved} onChange={(e) => setShowResolved(e.target.checked)} />
              Resolved
            </label>
          </div>
        </div>
        {filtered.length === 0 ? (
          <EmptyState icon={CircleCheck} title={items.length ? "No matches" : "No conflicts"}>
            When the same file changes on two devices, DevBridge keeps both versions and lists them here. It never picks a winner
            by timestamp.
          </EmptyState>
        ) : (
          <ul className="max-h-[60vh] divide-y divide-line overflow-y-auto">
            {filtered.map((c) => (
              <li key={c.id}>
                <button
                  type="button"
                  onClick={() => setSelected(c.id)}
                  aria-current={current === c.id}
                  className={`w-full px-3 py-2 text-left text-sm hover:bg-surface-2 ${current === c.id ? "bg-surface-2" : ""}`}
                >
                  <p className="truncate font-mono text-xs">{c.path}</p>
                  <p className="text-xs text-muted">
                    {c.workspaceName} · {CONFLICT_KIND[c.kind]} · {relativeTime(c.detectedAt)}
                  </p>
                  {c.state === "resolved" && <Badge tone="ok">{RESOLUTION_LABEL[c.resolution ?? ""] ?? "Resolved"}</Badge>}
                </button>
              </li>
            ))}
          </ul>
        )}
      </Card>
      {current ? (
        <ConflictPane
          key={current}
          id={current}
          onDone={() => {
            setSelected(null);
            void load();
          }}
        />
      ) : (
        <div />
      )}
    </div>
  );
}

function SideView({ side, label }: { side: ConflictDetail["local"]; label: string }) {
  if (side.kind === "missing") return <p className="p-3 text-sm text-muted">{label}: deleted</p>;
  if (side.kind === "binary")
    return (
      <p className="p-3 text-sm text-muted">
        {label}: binary or large file ({bytes(side.size)})
      </p>
    );
  return null;
}

function ConflictPane({ id, onDone }: { id: string; onDone: () => void }) {
  const { notify, refresh, overview } = useApp();
  const [d, setD] = useState<ConflictDetail | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [merging, setMerging] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api.conflictDetail(id).then(
      (x) => live && setD(x),
      (e) => live && setError(errorMessage(e)),
    );
    return () => {
      live = false;
    };
  }, [id]);

  const rows = useMemo(
    () => (d?.local.kind === "text" && d.remote.kind === "text" ? sideBySide(d.local.text, d.remote.text) : null),
    [d],
  );
  if (error) return <Notice tone="danger">{error}</Notice>;
  if (!d) return null;
  const here = overview?.device.name ?? "This device";
  const there = d.remoteDeviceName;
  const open = d.state === "open";

  const resolve = async (kind: "keep_local" | "keep_remote" | "save_both" | "merged", text?: string) => {
    setBusy(kind);
    try {
      await api.resolveConflict(id, kind, text);
      notify("ok", "Conflict resolved. The other device will receive the result.");
      await refresh();
      onDone();
    } catch (e) {
      notify("danger", errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <Card>
      <CardHeader
        title={d.path}
        subtitle={`${d.workspaceName} · ${CONFLICT_KIND[d.kind]} · detected ${relativeTime(d.detectedAt)}`}
      />
      <div className="space-y-3 px-4 py-3">
        {rows ? (
          <div className="max-h-[50vh] overflow-auto rounded border border-line font-mono text-xs">
            <div className="sticky top-0 grid grid-cols-2 border-b border-line bg-surface-2 text-[11px] font-sans font-medium">
              <div className="px-2 py-1">{here} (this device)</div>
              <div className="border-l border-line px-2 py-1">{there}</div>
            </div>
            {rows.map((r, i) => (
              <div key={i} className="grid grid-cols-2">
                <pre
                  className={`overflow-hidden px-2 whitespace-pre-wrap ${r.kind === "same" ? "" : r.left === null ? "bg-surface-2" : "bg-warn/15"}`}
                >
                  {r.left ?? " "}
                </pre>
                <pre
                  className={`overflow-hidden border-l border-line px-2 whitespace-pre-wrap ${r.kind === "same" ? "" : r.right === null ? "bg-surface-2" : "bg-info/15"}`}
                >
                  {r.right ?? " "}
                </pre>
              </div>
            ))}
          </div>
        ) : (
          <div className="grid grid-cols-2 gap-2 rounded border border-line">
            <SideView side={d.local} label={here} />
            <SideView side={d.remote} label={there} />
            {d.local.kind === "text" && (
              <p className="p-3 text-sm text-muted">
                {here}: text ({bytes(d.local.text.length)})
              </p>
            )}
            {d.remote.kind === "text" && (
              <p className="p-3 text-sm text-muted">
                {there}: text ({bytes(d.remote.text.length)})
              </p>
            )}
          </div>
        )}
        {open ? (
          <div className="flex flex-wrap gap-2">
            <Button busy={busy === "keep_local"} onClick={() => resolve("keep_local")}>
              Keep this device's
            </Button>
            <Button busy={busy === "keep_remote"} onClick={() => resolve("keep_remote")}>
              Keep {there}'s
            </Button>
            <Button
              busy={busy === "save_both"}
              disabled={d.kind === "remote_deleted"}
              onClick={() => resolve("save_both")}
              title="Keeps this device's file and saves the other version next to it"
            >
              Save both
            </Button>
            {d.local.kind === "text" && d.remote.kind === "text" && (
              <Button
                icon={GitMerge}
                onClick={() =>
                  setMerging(withMarkers((d.local as { text: string }).text, (d.remote as { text: string }).text, here, there))
                }
              >
                Merge…
              </Button>
            )}
          </div>
        ) : (
          <Notice tone="info">
            {RESOLUTION_LABEL[d.resolution ?? ""] ?? "Resolved"} {d.resolvedAt ? relativeTime(d.resolvedAt) : ""}.
          </Notice>
        )}
        <p className="text-xs text-muted">
          Both versions stay in the recovery history ({d.recovery.length} cop{d.recovery.length === 1 ? "y" : "ies"}) until you
          delete them, whichever you choose.
        </p>
      </div>
      {merging !== null && (
        <Modal
          open
          title={`Merge ${d.path}`}
          onClose={() => setMerging(null)}
          footer={
            <>
              <Button onClick={() => setMerging(null)}>Cancel</Button>
              <Button
                variant="primary"
                busy={busy === "merged"}
                disabled={hasMarkers(merging)}
                onClick={() => resolve("merged", merging)}
              >
                Save merged version
              </Button>
            </>
          }
        >
          <p className="text-sm text-muted">
            Edit the text below. Remove every conflict marker (&lt;&lt;&lt;&lt;&lt;&lt;&lt;, =======,
            &gt;&gt;&gt;&gt;&gt;&gt;&gt;) to save.
          </p>
          <textarea
            aria-label="Merged text"
            rows={18}
            spellCheck={false}
            className={`${inputClass} font-mono text-xs`}
            value={merging}
            onChange={(e) => setMerging(e.target.value)}
          />
        </Modal>
      )}
    </Card>
  );
}

function Issues() {
  const { overview, notify } = useApp();
  const [items, setItems] = useState<SyncIssue[]>([]);
  const load = useCallback(() => api.listIssues(null).then(setItems, (e) => notify("danger", errorMessage(e))), [notify]);
  useEffect(() => {
    void load();
  }, [load]);
  const name = (id: string) => overview?.workspaces.find((w) => w.id === id)?.name ?? "";
  return (
    <Card>
      <CardHeader title="Files that can't sync" subtitle="These stay on this computer only. Rename or remove them to sync." />
      {items.length === 0 ? (
        <EmptyState icon={CircleCheck} title="Everything can sync" />
      ) : (
        <ul className="divide-y divide-line">
          {items.map((i) => (
            <li key={i.workspaceId + i.path + i.kind} className="flex items-start gap-3 px-4 py-2 text-sm">
              <FileWarning size={16} className="mt-0.5 shrink-0 text-warn" />
              <div className="min-w-0 flex-1">
                <p className="truncate font-mono text-xs">{i.path}</p>
                <p className="text-xs text-muted">
                  {name(i.workspaceId)} · {ISSUE_LABEL[i.kind]}
                  {i.detail ? ` — ${i.detail}` : ""}
                </p>
              </div>
              {i.kind === "rejected" && (
                <Button variant="ghost" onClick={() => api.dismissIssue(i.workspaceId, i.path, i.kind).then(load)}>
                  Dismiss
                </Button>
              )}
            </li>
          ))}
        </ul>
      )}
    </Card>
  );
}

function Recovery() {
  const { overview, notify } = useApp();
  const [items, setItems] = useState<RecoveryItem[]>([]);
  const [removing, setRemoving] = useState<RecoveryItem | null>(null);
  const load = useCallback(() => api.listRecovery(null).then(setItems, (e) => notify("danger", errorMessage(e))), [notify]);
  useEffect(() => {
    void load();
  }, [load]);
  const name = (id: string) => overview?.workspaces.find((w) => w.id === id)?.name ?? "(removed project)";
  return (
    <Card>
      <CardHeader
        title="Recovery history"
        subtitle="Copies DevBridge kept before changing or deleting a file. Conflict copies are kept until you delete them; others expire after the retention period in Settings."
      />
      {items.length === 0 ? (
        <EmptyState icon={History} title="No recovery copies" />
      ) : (
        <ul className="divide-y divide-line">
          {items.map((r) => (
            <li key={r.id} className="flex flex-wrap items-center gap-3 px-4 py-2 text-sm">
              <div className="min-w-0 flex-1">
                <p className="truncate font-mono text-xs">{r.path}</p>
                <p className="text-xs text-muted">
                  {name(r.workspaceId)} · {RECOVERY_REASON[r.reason]} · {bytes(r.size)} · {relativeTime(r.createdAt)}
                  {r.pinned && " · kept until deleted"}
                </p>
              </div>
              <Button
                variant="ghost"
                icon={RotateCcw}
                onClick={() =>
                  api.restoreRecovery(r.id).then(
                    (p) => notify("ok", `Restored as ${p}.`),
                    (e) => notify("danger", errorMessage(e)),
                  )
                }
              >
                Restore copy
              </Button>
              <button
                type="button"
                aria-label={`Delete recovery copy of ${r.path}`}
                onClick={() => setRemoving(r)}
                className="rounded p-1 text-muted hover:text-danger"
              >
                <Trash size={14} />
              </button>
            </li>
          ))}
        </ul>
      )}
      {removing && (
        <Modal
          open
          title="Delete this recovery copy?"
          onClose={() => setRemoving(null)}
          footer={
            <>
              <Button onClick={() => setRemoving(null)}>Cancel</Button>
              <Button
                variant="danger"
                onClick={async () => {
                  const r = removing;
                  setRemoving(null);
                  try {
                    await api.removeRecovery(r.id);
                    await load();
                  } catch (e) {
                    notify("danger", errorMessage(e));
                  }
                }}
              >
                Delete permanently
              </Button>
            </>
          }
        >
          <p className="text-sm">
            The saved copy of <span className="font-mono">{removing.path}</span> will be permanently deleted. Your project files
            aren't affected.
          </p>
        </Modal>
      )}
    </Card>
  );
}
