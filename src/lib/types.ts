// Mirrors the serde output of the Rust types (crates/core/src/model.rs,
// src-tauri/src/commands.rs). Keep in sync when those change.

export type WorkspaceMode = "synced" | "remote";
export type SyncState = "local_only" | "paused" | "idle" | "syncing" | "offline" | "error";
export type DeviceStatus = "local" | "pending" | "paired" | "revoked";
export type DeviceRole = "peer" | "host" | "client";
export type OsKind = "macos" | "windows" | "linux" | "other";
export type ThemePreference = "system" | "light" | "dark";

export const WORKSPACE_COLORS = ["slate", "red", "orange", "amber", "green", "teal", "blue", "indigo", "violet", "pink"] as const;
export type WorkspaceColor = (typeof WORKSPACE_COLORS)[number];

export const WORKSPACE_ICONS = [
  "folder",
  "code",
  "globe",
  "server",
  "smartphone",
  "gamepad",
  "database",
  "package",
  "terminal",
  "book",
] as const;
export type WorkspaceIcon = (typeof WORKSPACE_ICONS)[number];

export interface Device {
  id: string;
  name: string;
  os: OsKind;
  osVersion: string | null;
  status: DeviceStatus;
  role: DeviceRole;
  isLocal: boolean;
  certFingerprint: string | null;
  lastSeenAt: number | null;
  createdAt: number;
}

export interface Workspace {
  id: string;
  name: string;
  rootPath: string;
  color: WorkspaceColor;
  icon: WorkspaceIcon;
  mode: WorkspaceMode;
  syncState: SyncState;
  activeDeviceId: string | null;
  lastSyncedAt: number | null;
  createdAt: number;
  updatedAt: number;
}

export interface GitRepoRecord {
  toplevel: string;
  lastBranch: string | null;
  lastHead: string | null;
  lastCheckedAt: number | null;
}

export interface WorkspaceSummary extends Workspace {
  openConflicts: number;
  pendingChanges: number;
  openIssues: number;
  git: GitRepoRecord | null;
  rootAvailable: boolean;
  paused: boolean;
  syncedDevices: number;
  onlineDevices: number;
}

export interface WorkspaceOffer {
  workspaceId: string;
  deviceId: string;
  deviceName: string;
  name: string;
  color: WorkspaceColor;
  icon: WorkspaceIcon;
  offeredAt: number;
}

export interface Overview {
  device: Device;
  workspaces: WorkspaceSummary[];
  pairedDeviceCount: number;
  onlineDeviceCount: number;
  offers: WorkspaceOffer[];
}

export interface RepoInfo {
  toplevel: string;
  isToplevel: boolean;
  branch: string | null;
  head: string | null;
}

export interface PathInspection {
  canonicalPath: string | null;
  suggestedName: string;
  suggestedColor: WorkspaceColor;
  git: RepoInfo | null;
  gitAvailable: boolean;
  warnings: string[];
  error: string | null;
}

export interface ImportRequest {
  path: string;
  name?: string;
  color?: WorkspaceColor;
  icon?: WorkspaceIcon;
  initGit: boolean;
}

export interface AppSettings {
  theme: ThemePreference;
  vscodePath: string | null;
  gitPath: string | null;
  excludePatterns: string[];
  secretOptins: string[];
  respectGitignore: boolean;
  maxConcurrentTransfers: number;
  bandwidthLimitKbps: number;
  maxFileSizeMb: number;
  launchAtLogin: boolean;
  startMinimized: boolean;
  notifications: boolean;
  recoveryRetentionDays: number;
  reconcileIntervalMinutes: number;
  lanEnabled: boolean;
  listenPort: number;
}

export type ToolSource = "custom" | "path" | "known_location" | "not_found";

export interface ToolStatus {
  name: string;
  found: boolean;
  path: string | null;
  version: string | null;
  source: ToolSource;
  hint: string | null;
}

export interface ToolReport {
  vscode: ToolStatus;
  git: ToolStatus;
  claudeCli: ToolStatus;
  claudeExtension: ToolStatus;
}

export interface AuditEvent {
  id: number;
  at: number;
  kind: string;
  workspaceId: string | null;
  deviceId: string | null;
  detail: Record<string, unknown> | null;
}

export interface Diagnostics {
  appVersion: string;
  os: string;
  arch: string;
  schemaVersion: number;
  dataDir: string;
  logsDir: string;
  gitFound: boolean;
  identityStorage: string;
  recoveryBytes: number;
  listenAddress: string | null;
}

/** Error shape returned by every command (src-tauri/src/error.rs). */
export interface CmdError {
  code: string;
  message: string;
}

export interface SyncDefaults {
  alwaysExcluded: string[];
  defaultExcludes: string[];
  secretPatterns: string[];
  secretExceptions: string[];
}

export interface DeviceView extends Device {
  online: boolean;
  shortFingerprint: string | null;
}

export interface EngineStatus {
  listenAddress: string | null;
  lan: boolean;
  port: number;
  onlinePeers: string[];
  fingerprint: string;
  shortFingerprint: string;
}

export interface DiscoveredDevice {
  deviceId: string;
  name: string;
  address: string;
}

export interface PairingWindow {
  code: string;
  expiresAt: number;
  addresses: string[];
  port: number;
  qrSvg: string | null;
}

export type EngineEvent =
  | { kind: "workspaceChanged"; workspaceId: string }
  | { kind: "devicesChanged" }
  | { kind: "peerOnline"; deviceId: string }
  | { kind: "peerOffline"; deviceId: string }
  | { kind: "pairingRequest"; requestId: string; deviceName: string; os: OsKind; verificationCode: string }
  | { kind: "pairingVerification"; verificationCode: string }
  | { kind: "pairingClosed"; reason: string }
  | { kind: "offerReceived"; workspaceId: string; deviceId: string; name: string }
  | { kind: "conflictDetected"; workspaceId: string; path: string }
  | { kind: "transferProgress"; workspaceId: string; path: string; done: number; total: number }
  | { kind: "error"; workspaceId: string | null; message: string }
  | { kind: "listenerChanged"; address: string | null; error: string | null }
  | { kind: "remote"; deviceId: string; message: unknown };

export interface RemoteEvent {
  kind: "request" | "granted" | "denied" | "revoked";
  deviceId: string;
  reason?: string;
}

export interface TransferJob {
  id: string;
  workspaceId: string;
  peerDeviceId: string;
  path: string;
  totalSize: number;
  bytesDone: number;
  state: string;
  attempts: number;
  lastError: string | null;
}

export interface ShareView {
  deviceId: string;
  deviceName: string;
  shared: boolean;
  joined: boolean;
  lastSyncAt: number | null;
  online: boolean;
}

export interface WorkspaceSync {
  paused: boolean;
  shares: ShareView[];
  transfers: TransferJob[];
  failedTransfers: TransferJob[];
  queuedChanges: number;
  openIssues: number;
}

export interface GitStatusEntry {
  path: string;
  origPath: string | null;
  index: string;
  worktree: string;
  kind: "ordinary" | "renamed" | "unmerged" | "untracked" | "ignored";
}

export interface GitStatus {
  branch: string | null;
  head: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  entries: GitStatusEntry[];
}

export interface GitCommit {
  hash: string;
  short: string;
  author: string;
  time: number;
  subject: string;
}

export interface GitOverview {
  isRepo: boolean;
  toplevel: string | null;
  status: GitStatus | null;
  log: GitCommit[];
  branches: { name: string; upstream: string | null; current: boolean }[];
  remotes: { name: string; url: string }[];
  autocrlf: string | null;
}

export interface ConflictView {
  id: string;
  workspaceId: string;
  path: string;
  kind: "content" | "local_deleted" | "remote_deleted";
  localVersionId: string | null;
  localHash: string | null;
  localSize: number | null;
  remoteVersionId: string;
  remoteHash: string | null;
  remoteSize: number | null;
  remoteDeviceId: string;
  state: "open" | "resolved";
  resolution: string | null;
  detectedAt: number;
  resolvedAt: number | null;
  workspaceName: string;
  remoteDeviceName: string;
}

export type ConflictSide = { kind: "text"; text: string } | { kind: "binary"; size: number } | { kind: "missing" };

export interface RecoveryItem {
  id: number;
  workspaceId: string;
  path: string;
  contentHash: string;
  size: number;
  reason: "conflict_local" | "conflict_remote" | "overwritten" | "deleted";
  conflictId: string | null;
  pinned: boolean;
  createdAt: number;
}

export interface ConflictDetail extends ConflictView {
  local: ConflictSide;
  remote: ConflictSide;
  recovery: RecoveryItem[];
}

export interface SyncIssue {
  workspaceId: string;
  path: string;
  kind: "unportable_path" | "case_collision" | "symlink" | "unreadable" | "rejected";
  detail: string;
  firstSeenAt: number;
}

export interface RemoteWorkspace {
  ws: string;
  name: string;
  path: string;
}

export interface RemoteStatus {
  hosting: string[];
  sshServerRunning: boolean;
  sshServerGuidance: string;
  grants: { deviceId: string; deviceName: string; grantedAt: number }[];
  pending: { deviceId: string; deviceName: string }[];
  hosts: {
    deviceId: string;
    deviceName: string;
    online: boolean;
    alias: string;
    sshUser: string;
    sshHost: string;
    hostOs: OsKind;
    workspaces: RemoteWorkspace[];
  }[];
  includeConfigured: boolean;
  remoteSshInstalled: boolean;
}
