import { GitMerge, LayoutDashboard, Monitor, Settings as SettingsIcon, X } from "lucide-react";
import type { LucideIcon } from "lucide-react";
import { AppProvider, useApp, type Route } from "./lib/app-context";
import { ProjectGlyph, Notice } from "./components/ui";
import { Dashboard } from "./views/Dashboard";
import { ProjectDetail } from "./views/ProjectDetail";
import { Devices } from "./views/Devices";
import { Conflicts } from "./views/Conflicts";
import { Settings } from "./views/Settings";
import { GlobalPrompts } from "./components/GlobalPrompts";
import logo from "./assets/logo.svg";

export default function App() {
  return (
    <AppProvider>
      <Shell />
    </AppProvider>
  );
}

function Shell() {
  const { route, loadError, toasts, dismiss } = useApp();
  return (
    <div className="flex h-full">
      <Sidebar />
      <main className="min-w-0 flex-1 overflow-y-auto">
        <div className="mx-auto max-w-6xl px-8 py-6">
          {loadError && (
            <div className="mb-4">
              <Notice tone="danger">{loadError}</Notice>
            </div>
          )}
          <View route={route} />
        </div>
      </main>
      <GlobalPrompts />
      <div aria-live="polite" className="pointer-events-none fixed right-4 bottom-4 z-50 flex w-80 flex-col gap-2">
        {toasts.map((t) => (
          <div
            key={t.id}
            role={t.tone === "danger" ? "alert" : "status"}
            className={`pointer-events-auto flex items-start gap-2 rounded-lg border bg-surface px-3 py-2 text-sm shadow-lg ${t.tone === "danger" ? "border-danger/40" : "border-ok/40"}`}
          >
            <span className="flex-1">{t.text}</span>
            <button type="button" aria-label="Dismiss" onClick={() => dismiss(t.id)} className="text-muted hover:text-fg">
              <X size={14} />
            </button>
          </div>
        ))}
      </div>
    </div>
  );
}

function View({ route }: { route: Route }) {
  switch (route.view) {
    case "dashboard":
      return <Dashboard />;
    case "project":
      return <ProjectDetail id={route.id} />;
    case "devices":
      return <Devices />;
    case "conflicts":
      return <Conflicts />;
    case "settings":
      return <Settings />;
  }
}

function Sidebar() {
  const { route, navigate, overview } = useApp();
  const conflicts = overview?.workspaces.reduce((n, w) => n + w.openConflicts, 0) ?? 0;
  const item = (r: Route, label: string, Icon: LucideIcon, badge?: number) => {
    const active = route.view === r.view && (r.view !== "project" || (route.view === "project" && route.id === r.id));
    return (
      <button
        type="button"
        onClick={() => navigate(r)}
        aria-current={active ? "page" : undefined}
        className={`flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-left text-sm ${active ? "bg-surface-2 font-medium" : "text-muted hover:bg-surface-2 hover:text-fg"}`}
      >
        <Icon size={16} />
        <span className="flex-1">{label}</span>
        {!!badge && <span className="rounded-full bg-danger px-1.5 text-xs text-white">{badge}</span>}
      </button>
    );
  };
  return (
    <nav aria-label="Main" className="flex w-60 shrink-0 flex-col border-r border-line bg-surface">
      <div className="flex items-center gap-2.5 px-4 py-4">
        <img src={logo} alt="" width={28} height={28} />
        <span className="font-semibold tracking-tight">DevBridge</span>
      </div>
      <div className="space-y-0.5 px-2">
        {item({ view: "dashboard" }, "Dashboard", LayoutDashboard)}
        {item({ view: "devices" }, "Devices", Monitor)}
        {item({ view: "conflicts" }, "Conflicts", GitMerge, conflicts)}
        {item({ view: "settings" }, "Settings", SettingsIcon)}
      </div>
      <p className="mt-6 px-4 pb-1 text-xs font-medium tracking-wide text-muted uppercase">Projects</p>
      <div className="min-h-0 flex-1 space-y-0.5 overflow-y-auto px-2 pb-4">
        {overview?.workspaces.map((w) => (
          <button
            key={w.id}
            type="button"
            onClick={() => navigate({ view: "project", id: w.id })}
            aria-current={route.view === "project" && route.id === w.id ? "page" : undefined}
            className={`flex w-full items-center gap-2 rounded-md px-2 py-1 text-left text-sm ${route.view === "project" && route.id === w.id ? "bg-surface-2 font-medium" : "text-muted hover:bg-surface-2 hover:text-fg"}`}
          >
            <ProjectGlyph color={w.color} icon={w.icon} size={22} />
            <span className="truncate">{w.name}</span>
          </button>
        ))}
        {overview && overview.workspaces.length === 0 && <p className="px-2 text-xs text-muted">No projects yet.</p>}
      </div>
      {overview && (
        <div className="border-t border-line px-4 py-3 text-xs text-muted">
          This device: <span className="font-medium text-fg">{overview.device.name}</span>
          <br />
          {overview.pairedDeviceCount > 0
            ? `${overview.onlineDeviceCount} of ${overview.pairedDeviceCount} paired device(s) online`
            : "No paired devices"}
        </div>
      )}
    </nav>
  );
}
