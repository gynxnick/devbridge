import { invoke } from "@tauri-apps/api/core";
import type {
  AppSettings,
  ConflictDetail,
  ConflictView,
  DeviceView,
  DiscoveredDevice,
  EngineStatus,
  GitOverview,
  PairingWindow,
  RecoveryItem,
  RemoteStatus,
  SyncIssue,
  WorkspaceOffer,
  WorkspaceSync,
  AuditEvent,
  CmdError,
  Device,
  Diagnostics,
  ImportRequest,
  Overview,
  PathInspection,
  RepoInfo,
  SyncDefaults,
  ToolReport,
  Workspace,
  WorkspaceColor,
  WorkspaceIcon,
} from "./types";

/** Typed wrappers around the Tauri commands in src-tauri/src/commands.rs. */
export const api = {
  getOverview: () => invoke<Overview>("get_overview"),
  inspectPath: (path: string) => invoke<PathInspection>("inspect_path", { path }),
  importWorkspace: (request: ImportRequest) => invoke<Workspace>("import_workspace", { request }),
  updateWorkspace: (id: string, name: string, color: WorkspaceColor, icon: WorkspaceIcon) =>
    invoke<Workspace>("update_workspace", { id, name, color, icon }),
  removeWorkspace: (id: string) => invoke<void>("remove_workspace", { id }),
  refreshGit: (id: string) => invoke<RepoInfo | null>("refresh_git", { id }),
  openInVscode: (id: string) => invoke<void>("open_in_vscode", { id }),
  openTerminal: (id: string) => invoke<void>("open_terminal", { id }),
  listDevices: () => invoke<DeviceView[]>("list_devices"),
  renameLocalDevice: (name: string) => invoke<Device>("rename_local_device", { name }),
  getSettings: () => invoke<AppSettings>("get_settings"),
  saveSettings: (settings: AppSettings) => invoke<AppSettings>("save_settings", { settings }),
  detectTools: () => invoke<ToolReport>("detect_tools"),
  listActivity: (limit?: number) => invoke<AuditEvent[]>("list_activity", { limit }),
  diagnostics: () => invoke<Diagnostics>("diagnostics"),
  exportDiagnostics: (path: string) => invoke<void>("export_diagnostics", { path }),
  syncDefaults: () => invoke<SyncDefaults>("sync_defaults"),

  // Git
  gitOverview: (id: string) => invoke<GitOverview>("git_overview", { id }),
  gitStage: (id: string, paths: string[]) => invoke<void>("git_stage", { id, paths }),
  gitUnstage: (id: string, paths: string[]) => invoke<void>("git_unstage", { id, paths }),
  gitCommit: (id: string, message: string) => invoke<string>("git_commit", { id, message }),
  gitFetch: (id: string) => invoke<void>("git_fetch", { id }),
  gitPull: (id: string, confirmed: boolean) => invoke<void>("git_pull", { id, confirmed }),
  gitPush: (id: string, remote?: string) => invoke<void>("git_push", { id, remote }),
  gitSwitch: (id: string, branch: string, create: boolean, confirmed: boolean) =>
    invoke<void>("git_switch", { id, branch, create, confirmed }),
  gitRemoteAdd: (id: string, name: string, url: string) => invoke<void>("git_remote_add", { id, name, url }),
  gitRemoteSetUrl: (id: string, name: string, url: string) => invoke<void>("git_remote_set_url", { id, name, url }),
  gitRemoteRemove: (id: string, name: string) => invoke<void>("git_remote_remove", { id, name }),
  gitCatchUp: (id: string) => invoke<{ from: string; to: string; files: number }>("git_catch_up", { id }),
  gitInit: (id: string) => invoke<void>("git_init", { id }),

  // Sync
  workspaceSync: (id: string) => invoke<WorkspaceSync>("workspace_sync", { id }),
  setPaused: (id: string, paused: boolean) => invoke<void>("set_paused", { id, paused }),
  rescan: (id: string) => invoke<void>("rescan", { id }),
  retryTransfers: (id: string) => invoke<void>("retry_transfers", { id }),
  shareWorkspace: (id: string, deviceId: string) => invoke<void>("share_workspace", { id, deviceId }),
  unshareWorkspace: (id: string, deviceId: string) => invoke<void>("unshare_workspace", { id, deviceId }),
  listOffers: () => invoke<WorkspaceOffer[]>("list_offers"),
  joinOffer: (workspaceId: string, deviceId: string, path?: string) =>
    invoke<void>("join_offer", { workspaceId, deviceId, path }),
  declineOffer: (workspaceId: string, deviceId: string) => invoke<void>("decline_offer", { workspaceId, deviceId }),

  // Devices & pairing
  engineStatus: () => invoke<EngineStatus>("engine_status"),
  listDiscovered: () => invoke<DiscoveredDevice[]>("list_discovered"),
  pairingOpen: () => invoke<PairingWindow>("pairing_open"),
  pairingClose: () => invoke<void>("pairing_close"),
  pairingCurrent: () => invoke<PairingWindow | null>("pairing_current"),
  pairingDecide: (requestId: string, approve: boolean) => invoke<void>("pairing_decide", { requestId, approve }),
  pairJoin: (address: string, code: string) => invoke<Device>("pair_join", { address, code }),
  revokeDevice: (id: string) => invoke<void>("revoke_device", { id }),

  // Conflicts, issues, recovery
  listConflicts: (workspaceId: string | null, includeResolved: boolean) =>
    invoke<ConflictView[]>("list_conflicts", { workspaceId, includeResolved }),
  conflictDetail: (id: string) => invoke<ConflictDetail>("conflict_detail", { id }),
  resolveConflict: (id: string, resolution: "keep_local" | "keep_remote" | "save_both" | "merged", mergedText?: string) =>
    invoke<void>("resolve_conflict", { id, resolution, mergedText }),
  listIssues: (workspaceId: string | null) => invoke<SyncIssue[]>("list_issues", { workspaceId }),
  dismissIssue: (workspaceId: string, path: string, kind: string) => invoke<void>("dismiss_issue", { workspaceId, path, kind }),
  listRecovery: (workspaceId: string | null) => invoke<RecoveryItem[]>("list_recovery", { workspaceId }),
  restoreRecovery: (id: number) => invoke<string>("restore_recovery", { id }),
  removeRecovery: (id: number) => invoke<void>("remove_recovery", { id }),

  // Remote workspace
  remoteStatus: () => invoke<RemoteStatus>("remote_status"),
  remoteSetHosting: (workspaceId: string, enabled: boolean) => invoke<void>("remote_set_hosting", { workspaceId, enabled }),
  remoteRequestAccess: (deviceId: string) => invoke<void>("remote_request_access", { deviceId }),
  remoteDecide: (deviceId: string, allow: boolean) => invoke<void>("remote_decide", { deviceId, allow }),
  remoteRevoke: (deviceId: string) => invoke<void>("remote_revoke", { deviceId }),
  remoteOpen: (deviceId: string, workspaceId: string, allowInclude: boolean) =>
    invoke<void>("remote_open", { deviceId, workspaceId, allowInclude }),
  remoteCheck: (deviceId: string) => invoke<{ devbridgeOnline: boolean; sshReachable: boolean }>("remote_check", { deviceId }),
};

export function errorCode(e: unknown): string | null {
  if (typeof e === "object" && e !== null && "code" in e) return String((e as CmdError).code);
  return null;
}

export function errorMessage(e: unknown): string {
  if (typeof e === "object" && e !== null && "message" in e) {
    return String((e as CmdError).message);
  }
  return String(e);
}
