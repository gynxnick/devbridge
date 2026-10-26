import type { OsKind, SyncIssue, SyncState } from "./types";

export function relativeTime(ms: number | null, now: number = Date.now()): string {
  if (ms === null) return "Never";
  const s = Math.round((now - ms) / 1000);
  if (s < 45) return "Just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  if (d < 30) return `${d} d ago`;
  return new Date(ms).toLocaleDateString();
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

export function osLabel(os: OsKind): string {
  return { macos: "macOS", windows: "Windows", linux: "Linux", other: "Other" }[os];
}

export const SYNC_STATE: Record<SyncState, { label: string; tone: "ok" | "warn" | "danger" | "info" | "muted"; help: string }> = {
  local_only: {
    label: "Local only",
    tone: "muted",
    help: "No paired device syncs this project yet. Share it from the project page.",
  },
  paused: { label: "Paused", tone: "warn", help: "Sync is paused. Local changes are recorded and sent when you resume." },
  idle: { label: "Up to date", tone: "ok", help: "All known changes have been exchanged with connected devices." },
  syncing: { label: "Syncing", tone: "info", help: "Changes are being transferred." },
  offline: { label: "Offline", tone: "muted", help: "The devices this project syncs with aren't connected. Changes are queued." },
  error: {
    label: "Needs attention",
    tone: "danger",
    help: "The project folder is unavailable. Reconnect the disk or move it back.",
  },
};

const AUDIT_LABELS: Record<string, string> = {
  "device.local_created": "This device was registered",
  "device.renamed": "Device renamed",
  "device.paired": "Device paired",
  "device.revoked": "Device removed",
  "workspace.imported": "Project added",
  "workspace.removed": "Project removed from DevBridge (files kept)",
  "workspace.shared": "Project shared with a device",
  "workspace.unshared": "Project no longer shared with a device",
  "settings.secret_optins_changed": "Secret-file sync exclusions changed",
  "conflict.detected": "Conflict detected",
  "conflict.resolved": "Conflict resolved",
  "pairing.opened": "Pairing window opened",
  "sync.paused": "Sync paused",
  "sync.resumed": "Sync resumed",
  "recovery.restored": "Recovery copy restored",
  "recovery.removed": "Recovery copy deleted",
  "diagnostics.exported": "Diagnostics exported",
  "remote.hosting_enabled": "Remote access enabled for a project",
  "remote.hosting_disabled": "Remote access disabled for a project",
  "remote.access_requested": "Remote access requested",
  "remote.access_granted": "Remote access granted",
  "remote.access_revoked": "Remote access revoked",
  "remote.ssh_config_include_added": "SSH config Include line added",
};

export function auditLabel(kind: string): string {
  return AUDIT_LABELS[kind] ?? kind;
}

export function shortHash(h: string | null): string {
  return h ? h.slice(0, 7) : "—";
}

export const ISSUE_LABEL: Record<SyncIssue["kind"], string> = {
  unportable_path: "Name not allowed on Windows",
  case_collision: "Name differs only by letter case",
  symlink: "Symbolic link (not synced)",
  unreadable: "Couldn't be read",
  rejected: "Not accepted from the other device",
};

export const CONFLICT_KIND: Record<string, string> = {
  content: "Edited on both devices",
  local_deleted: "Deleted here, edited there",
  remote_deleted: "Edited here, deleted there",
};

export const RESOLUTION_LABEL: Record<string, string> = {
  keep_local: "Kept this device's version",
  keep_remote: "Kept the other device's version",
  save_both: "Saved both",
  merged: "Merged",
  resolved_elsewhere: "Resolved on the other device",
};

export const RECOVERY_REASON: Record<string, string> = {
  conflict_local: "This device's side of a conflict",
  conflict_remote: "Other device's side of a conflict",
  overwritten: "Replaced by a synced update",
  deleted: "Deleted by a synced update",
};
