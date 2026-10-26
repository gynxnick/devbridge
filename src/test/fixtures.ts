import { mockIPC } from "@tauri-apps/api/mocks";
import type { AppSettings, Overview, WorkspaceSummary } from "../lib/types";

export const device = {
  id: "01JDEVICE0000000000000000A",
  name: "Studio Mac",
  os: "macos" as const,
  osVersion: "26.0",
  status: "local" as const,
  role: "peer" as const,
  isLocal: true,
  certFingerprint: null,
  lastSeenAt: null,
  createdAt: Date.now() - 86_400_000,
};

export function workspace(overrides: Partial<WorkspaceSummary> = {}): WorkspaceSummary {
  return {
    id: "01JWORKSPACE00000000000001",
    name: "Website",
    rootPath: "/Users/me/Developer/website",
    color: "blue",
    icon: "code",
    mode: "synced",
    syncState: "local_only",
    activeDeviceId: device.id,
    lastSyncedAt: null,
    createdAt: 0,
    updatedAt: 0,
    openConflicts: 0,
    pendingChanges: 0,
    openIssues: 0,
    git: { toplevel: "/Users/me/Developer/website", lastBranch: "main", lastHead: "abcdef1234567", lastCheckedAt: null },
    rootAvailable: true,
    paused: false,
    syncedDevices: 0,
    onlineDevices: 0,
    ...overrides,
  };
}

export const defaultSettings: AppSettings = {
  theme: "system",
  vscodePath: null,
  gitPath: null,
  excludePatterns: [],
  secretOptins: [],
  respectGitignore: true,
  maxConcurrentTransfers: 4,
  bandwidthLimitKbps: 0,
  maxFileSizeMb: 2048,
  launchAtLogin: false,
  startMinimized: false,
  notifications: true,
  recoveryRetentionDays: 30,
  reconcileIntervalMinutes: 10,
  lanEnabled: false,
  listenPort: 47821,
};

export function overviewOf(workspaces: WorkspaceSummary[], extra: Partial<Overview> = {}): Overview {
  return { device, workspaces, pairedDeviceCount: 0, onlineDeviceCount: 0, offers: [], ...extra };
}

export const peer = {
  id: "01JPEERPEERPEERPEERPEER000",
  name: "Gaming PC",
  os: "windows" as const,
  osVersion: "11",
  status: "paired" as const,
  role: "peer" as const,
  isLocal: false,
  certFingerprint: "ab".repeat(32),
  lastSeenAt: Date.now() - 60_000,
  createdAt: 0,
  online: true,
  shortFingerprint: "abab abab abab abab",
};

type Handler = (args: Record<string, unknown>) => unknown;

/** Installs an IPC mock with sensible defaults; returns the list of calls made. */
export function mockBackend(overview: Overview, handlers: Record<string, Handler> = {}) {
  const calls: { cmd: string; args: Record<string, unknown> }[] = [];
  const defaults: Record<string, Handler> = {
    get_overview: () => overview,
    get_settings: () => defaultSettings,
    list_activity: () => [],
    list_devices: () => [{ ...device, online: true, shortFingerprint: null }],
    engine_status: () => ({
      listenAddress: "127.0.0.1:47821",
      lan: false,
      port: 47821,
      onlinePeers: [],
      fingerprint: "f".repeat(64),
      shortFingerprint: "ffff ffff ffff ffff",
    }),
    list_discovered: () => [],
    remote_status: () => ({
      hosting: [],
      sshServerRunning: false,
      sshServerGuidance: "Turn on Remote Login.",
      grants: [],
      pending: [],
      hosts: [],
      includeConfigured: false,
      remoteSshInstalled: true,
    }),
    workspace_sync: () => ({ paused: false, shares: [], transfers: [], failedTransfers: [], queuedChanges: 0, openIssues: 0 }),
    list_conflicts: () => [],
    list_issues: () => [],
    list_recovery: () => [],
    detect_tools: () => ({
      vscode: {
        name: "Visual Studio Code",
        found: true,
        path: "/usr/local/bin/code",
        version: "1.105.0",
        source: "path",
        hint: null,
      },
      git: { name: "Git", found: true, path: "/usr/bin/git", version: "2.54.0", source: "path", hint: null },
      claudeCli: { name: "Claude Code CLI", found: false, path: null, version: null, source: "not_found", hint: "Optional." },
      claudeExtension: {
        name: "Claude Code extension",
        found: true,
        path: "/x",
        version: "2.1.310",
        source: "known_location",
        hint: null,
      },
    }),
    sync_defaults: () => ({
      alwaysExcluded: [".git/"],
      defaultExcludes: ["node_modules/"],
      secretPatterns: [".env", "*.pem"],
      secretExceptions: [],
    }),
    diagnostics: () => ({
      appVersion: "0.1.0",
      os: "Mac OS 26.0",
      arch: "aarch64",
      schemaVersion: 1,
      dataDir: "/tmp/d",
      logsDir: "/tmp/d/logs",
      gitFound: true,
      identityStorage: "system keychain",
      recoveryBytes: 0,
      listenAddress: null,
    }),
  };
  mockIPC(
    (cmd, args) => {
      calls.push({ cmd, args: (args ?? {}) as Record<string, unknown> });
      const h = handlers[cmd] ?? defaults[cmd];
      if (!h) throw new Error(`unmocked command ${cmd}`);
      return h((args ?? {}) as Record<string, unknown>);
    },
    { shouldMockEvents: true },
  );
  return calls;
}
