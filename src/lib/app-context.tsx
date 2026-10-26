import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, errorMessage } from "./api";
import type { AppSettings, EngineEvent, Overview, RemoteEvent, ThemePreference } from "./types";

export type Route =
  | { view: "dashboard" }
  | { view: "project"; id: string }
  | { view: "devices" }
  | { view: "conflicts"; conflictId?: string }
  | { view: "settings" };

export interface Toast {
  id: number;
  tone: "ok" | "danger";
  text: string;
}

export interface PairingPrompt {
  requestId: string;
  deviceName: string;
  verificationCode: string;
}

type Listener = (e: EngineEvent) => void;

interface AppContextValue {
  overview: Overview | null;
  loadError: string | null;
  refresh: () => Promise<void>;
  settings: AppSettings | null;
  setSettings: (s: AppSettings) => void;
  route: Route;
  navigate: (r: Route) => void;
  toasts: Toast[];
  notify: (tone: Toast["tone"], text: string) => void;
  dismiss: (id: number) => void;
  /** Subscribe to engine events; returns an unsubscribe function. */
  onEngine: (l: Listener) => () => void;
  pairingPrompt: PairingPrompt | null;
  clearPairingPrompt: () => void;
  remotePrompt: RemoteEvent | null;
  clearRemotePrompt: () => void;
}

const Ctx = createContext<AppContextValue | null>(null);

export function useApp(): AppContextValue {
  const v = useContext(Ctx);
  if (!v) throw new Error("useApp must be used inside <AppProvider>");
  return v;
}

/** Calls `cb` for engine events while the component is mounted. */
export function useEngineEvents(cb: Listener) {
  const { onEngine } = useApp();
  const ref = useRef(cb);
  useEffect(() => {
    ref.current = cb;
  });
  useEffect(() => onEngine((e) => ref.current(e)), [onEngine]);
}

let toastSeq = 0;

export function AppProvider({ children }: { children: ReactNode }) {
  const [overview, setOverview] = useState<Overview | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [route, setRoute] = useState<Route>({ view: "dashboard" });
  const [toasts, setToasts] = useState<Toast[]>([]);
  const [pairingPrompt, setPairingPrompt] = useState<PairingPrompt | null>(null);
  const [remotePrompt, setRemotePrompt] = useState<RemoteEvent | null>(null);
  const listeners = useRef(new Set<Listener>());
  const refreshTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const refresh = useCallback(async () => {
    try {
      setOverview(await api.getOverview());
      setLoadError(null);
    } catch (e) {
      setLoadError(errorMessage(e));
    }
  }, []);

  const dismiss = useCallback((id: number) => setToasts((t) => t.filter((x) => x.id !== id)), []);
  const notify = useCallback(
    (tone: Toast["tone"], text: string) => {
      const id = ++toastSeq;
      setToasts((t) => [...t.slice(-4), { id, tone, text }]);
      setTimeout(() => dismiss(id), tone === "danger" ? 8000 : 4000);
    },
    [dismiss],
  );

  const onEngine = useCallback((l: Listener) => {
    listeners.current.add(l);
    return () => {
      listeners.current.delete(l);
    };
  }, []);

  useEffect(() => {
    let live = true;
    const fail = (e: unknown) => live && setLoadError(errorMessage(e));
    api.getOverview().then((o) => live && setOverview(o), fail);
    api.getSettings().then((s) => live && setSettings(s), fail);
    return () => {
      live = false;
    };
  }, []);

  // Engine events: fan out to subscribers, refresh the overview (debounced),
  // and surface the ones that need the user's attention.
  useEffect(() => {
    const scheduleRefresh = () => {
      if (refreshTimer.current) clearTimeout(refreshTimer.current);
      refreshTimer.current = setTimeout(() => void refresh(), 250);
    };
    const unlisten: Promise<() => void>[] = [
      listen<EngineEvent>("engine", ({ payload: e }) => {
        listeners.current.forEach((l) => l(e));
        switch (e.kind) {
          case "pairingRequest":
            setPairingPrompt({ requestId: e.requestId, deviceName: e.deviceName, verificationCode: e.verificationCode });
            break;
          case "conflictDetected":
            notify("danger", `Conflict: ${e.path} changed on two devices. Open Conflicts to decide.`);
            scheduleRefresh();
            break;
          case "offerReceived":
            notify("ok", `“${e.name}” was shared with you. Join it from the dashboard.`);
            scheduleRefresh();
            break;
          case "error":
            notify("danger", e.message);
            scheduleRefresh();
            break;
          case "listenerChanged":
            if (e.error) notify("danger", e.error);
            break;
          case "transferProgress":
          case "pairingVerification":
          case "remote":
            break;
          default:
            scheduleRefresh();
        }
      }),
      listen<RemoteEvent>("remote", ({ payload }) => {
        if (payload.kind === "request") setRemotePrompt(payload);
        else if (payload.kind === "granted") notify("ok", "Remote access granted. You can now open the host's projects.");
        else if (payload.kind === "denied") notify("danger", payload.reason ?? "Remote access was declined.");
        else if (payload.kind === "revoked") notify("danger", "Remote access to a host was revoked.");
      }),
    ];
    return () => {
      unlisten.forEach((p) => p.then((u) => u()).catch(() => {}));
      if (refreshTimer.current) clearTimeout(refreshTimer.current);
    };
  }, [refresh, notify]);

  useTheme(settings?.theme ?? "system");

  const value = useMemo(
    () => ({
      overview,
      loadError,
      refresh,
      settings,
      setSettings,
      route,
      navigate: setRoute,
      toasts,
      notify,
      dismiss,
      onEngine,
      pairingPrompt,
      clearPairingPrompt: () => setPairingPrompt(null),
      remotePrompt,
      clearRemotePrompt: () => setRemotePrompt(null),
    }),
    [overview, loadError, refresh, settings, route, toasts, notify, dismiss, onEngine, pairingPrompt, remotePrompt],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

function useTheme(pref: ThemePreference) {
  useEffect(() => {
    const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
    const apply = () => {
      const dark = pref === "dark" || (pref === "system" && !!mq?.matches);
      document.documentElement.classList.toggle("dark", dark);
    };
    apply();
    mq?.addEventListener?.("change", apply);
    return () => mq?.removeEventListener?.("change", apply);
  }, [pref]);
}
