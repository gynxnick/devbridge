import { useCallback, useEffect, useState } from "react";
import { CodeXml, KeyRound, Laptop, Link2, LogIn, Pencil, Radar, Server, ShieldOff, TriangleAlert } from "lucide-react";
import { api, errorCode, errorMessage } from "../lib/api";
import { useApp, useEngineEvents } from "../lib/app-context";
import { osLabel, relativeTime } from "../lib/format";
import type { DeviceView, DiscoveredDevice, EngineStatus, PairingWindow, RemoteStatus } from "../lib/types";
import { Badge, Button, Card, CardHeader, Dot, EmptyState, Field, Modal, Notice, inputClass } from "../components/ui";

export function Devices() {
  const { refresh, notify, settings, setSettings, navigate } = useApp();
  const [devices, setDevices] = useState<DeviceView[] | null>(null);
  const [status, setStatus] = useState<EngineStatus | null>(null);
  const [renaming, setRenaming] = useState<DeviceView | null>(null);
  const [pairing, setPairing] = useState(false);
  const [joining, setJoining] = useState(false);
  const [revoking, setRevoking] = useState<DeviceView | null>(null);

  const load = useCallback(async () => {
    try {
      const [d, s] = await Promise.all([api.listDevices(), api.engineStatus()]);
      setDevices(d);
      setStatus(s);
    } catch (e) {
      notify("danger", errorMessage(e));
    }
  }, [notify]);

  useEffect(() => {
    let live = true;
    Promise.all([api.listDevices(), api.engineStatus()]).then(
      ([d, s]) => {
        if (live) {
          setDevices(d);
          setStatus(s);
        }
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, []);

  useEngineEvents((e) => {
    if (["devicesChanged", "peerOnline", "peerOffline", "listenerChanged"].includes(e.kind)) void load();
  });

  const local = devices?.find((d) => d.isLocal);
  const others = devices?.filter((d) => !d.isLocal && d.status === "paired") ?? [];

  const enableLan = async () => {
    if (!settings) return;
    try {
      const saved = await api.saveSettings({ ...settings, lanEnabled: true });
      setSettings(saved);
      await load();
      notify("ok", "This computer now accepts connections from paired devices on your network.");
    } catch (e) {
      notify("danger", errorMessage(e));
    }
  };

  return (
    <div className="space-y-6">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-xl font-semibold tracking-tight">Devices</h1>
          <p className="text-sm text-muted">Computers that can sync your projects or open them remotely.</p>
        </div>
        <div className="flex gap-2">
          <Button icon={LogIn} onClick={() => setJoining(true)}>
            Join a device
          </Button>
          <Button variant="primary" icon={Link2} onClick={() => setPairing(true)}>
            Pair new device
          </Button>
        </div>
      </header>

      {status && !status.lan && (
        <Notice tone="warn">
          <p className="font-medium">Network connections are off.</p>
          <p className="mt-1">
            DevBridge only listens on this computer (loopback), so your other computer can't connect to it. Turn this on to pair
            and sync over your local network. Connections are always encrypted and only paired devices are accepted.
          </p>
          <div className="mt-2">
            <Button onClick={enableLan}>Allow connections from paired devices on this network</Button>
          </div>
        </Notice>
      )}

      {local && (
        <Card>
          <CardHeader
            title="This device"
            action={
              <Badge tone="ok">
                <Dot tone="ok" />
                Online
              </Badge>
            }
          />
          <div className="flex flex-wrap items-center gap-3 px-4 py-3">
            <Laptop size={22} className="text-muted" />
            <div className="flex-1">
              <p className="flex items-center gap-1 font-medium">
                {local.name}
                <button
                  type="button"
                  aria-label="Rename this device"
                  onClick={() => setRenaming(local)}
                  className="rounded p-1 text-muted hover:bg-surface-2 hover:text-fg"
                >
                  <Pencil size={14} />
                </button>
              </p>
              <p className="text-xs text-muted">
                {osLabel(local.os)} {local.osVersion ?? ""}
              </p>
            </div>
            {status && (
              <dl className="grid grid-cols-[auto_1fr] gap-x-3 text-xs">
                <dt className="text-muted">Listening on</dt>
                <dd className="font-mono">{status.listenAddress ?? "not listening"}</dd>
                <dt className="text-muted">Identity</dt>
                <dd className="font-mono" title={status.fingerprint}>
                  {status.shortFingerprint}
                </dd>
              </dl>
            )}
          </div>
        </Card>
      )}

      <Card>
        <CardHeader title="Paired devices" />
        {others.length === 0 ? (
          <EmptyState icon={Link2} title="No paired devices">
            On one computer choose <strong>Pair new device</strong> to get a code; on the other choose{" "}
            <strong>Join a device</strong> and enter it. Nothing is trusted until you approve the request.
          </EmptyState>
        ) : (
          <ul className="divide-y divide-line">
            {others.map((d) => (
              <li key={d.id} className="flex flex-wrap items-center gap-3 px-4 py-3 text-sm">
                <Dot tone={d.online ? "ok" : "muted"} />
                <div className="min-w-0 flex-1">
                  <p className="font-medium">{d.name}</p>
                  <p className="text-xs text-muted">
                    {osLabel(d.os)} · {d.online ? "Connected" : `Last seen ${relativeTime(d.lastSeenAt)}`}
                    {d.shortFingerprint && <span className="font-mono"> · {d.shortFingerprint}</span>}
                  </p>
                </div>
                <Button variant="ghost" icon={ShieldOff} onClick={() => setRevoking(d)}>
                  Remove
                </Button>
              </li>
            ))}
          </ul>
        )}
      </Card>

      <RemoteAccess devices={others} onChanged={() => void refresh()} goSettings={() => navigate({ view: "settings" })} />

      {renaming && (
        <RenameDialog
          device={renaming}
          onClose={() => setRenaming(null)}
          onSaved={async () => {
            setRenaming(null);
            await load();
            await refresh();
          }}
        />
      )}
      {pairing && (
        <PairHostDialog
          lan={status?.lan ?? false}
          onClose={() => {
            setPairing(false);
            void load();
          }}
        />
      )}
      {joining && (
        <JoinDialog
          onClose={() => {
            setJoining(false);
            void load();
            void refresh();
          }}
        />
      )}
      {revoking && (
        <Modal
          open
          title={`Remove ${revoking.name}?`}
          onClose={() => setRevoking(null)}
          footer={
            <>
              <Button onClick={() => setRevoking(null)}>Cancel</Button>
              <Button
                variant="danger"
                onClick={async () => {
                  const d = revoking;
                  setRevoking(null);
                  try {
                    await api.revokeDevice(d.id);
                    notify("ok", `${d.name} was removed. It can no longer connect.`);
                    await load();
                    await refresh();
                  } catch (e) {
                    notify("danger", errorMessage(e));
                  }
                }}
              >
                Remove device
              </Button>
            </>
          }
        >
          <ul className="list-disc space-y-1 pl-5 text-sm">
            <li>Its connection is closed and its certificate is no longer trusted.</li>
            <li>Projects stop syncing with it. Files on both computers stay as they are.</li>
            <li>Any remote-access SSH key DevBridge added for it is removed.</li>
          </ul>
          <p className="text-xs text-muted">To connect again later, pair from scratch.</p>
        </Modal>
      )}
    </div>
  );
}

function PairHostDialog({ lan, onClose }: { lan: boolean; onClose: () => void }) {
  const { notify } = useApp();
  const [win, setWin] = useState<PairingWindow | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [closedReason, setClosedReason] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    let live = true;
    api.pairingOpen().then(
      (w) => live && setWin(w),
      (e) => live && setError(errorMessage(e)),
    );
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => {
      live = false;
      clearInterval(t);
    };
  }, []);

  useEngineEvents((e) => {
    if (e.kind === "pairingClosed") {
      setClosedReason(e.reason);
      if (e.reason.startsWith("Paired")) notify("ok", e.reason);
    }
  });

  const close = () => {
    if (!closedReason) void api.pairingClose();
    onClose();
  };
  const left = win ? Math.max(0, Math.round((win.expiresAt - now) / 1000)) : 0;

  return (
    <Modal
      open
      title="Pair a new device"
      onClose={close}
      footer={<Button onClick={close}>{closedReason ? "Done" : "Cancel"}</Button>}
    >
      {error && <Notice tone="danger">{error}</Notice>}
      {closedReason ? (
        <Notice tone={closedReason.startsWith("Paired") ? "info" : "warn"}>{closedReason}</Notice>
      ) : (
        win && (
          <>
            <p className="text-sm">
              On your other computer, open DevBridge → Devices → <strong>Join a device</strong>, then enter:
            </p>
            <div className="flex items-center gap-4">
              <div className="flex-1 space-y-2">
                <div className="rounded-lg bg-surface-2 py-3 text-center">
                  <p className="text-xs text-muted">Pairing code</p>
                  <p className="font-mono text-3xl font-semibold tracking-widest">{win.code}</p>
                </div>
                <p className="text-sm">
                  Address: <span className="font-mono">{win.addresses[0] ?? `port ${win.port}`}</span>
                </p>
                <p className="text-xs text-muted">
                  Expires in {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")} · single use · 3 attempts
                </p>
              </div>
              {win.qrSvg && (
                // SVG generated locally by DevBridge from the code and address.
                <div
                  className="w-36 shrink-0 rounded bg-white p-1"
                  aria-label="Pairing QR code"
                  dangerouslySetInnerHTML={{ __html: win.qrSvg }}
                />
              )}
            </div>
            {!lan && (
              <Notice tone="warn">
                <TriangleAlert size={14} className="mr-1 inline" />
                Network connections are off, so only another DevBridge on <em>this</em> computer can use this code.
              </Notice>
            )}
            <p className="text-xs text-muted">
              You'll be asked to approve the request and compare a 6-digit verification code on both screens.
            </p>
          </>
        )
      )}
    </Modal>
  );
}

function JoinDialog({ onClose }: { onClose: () => void }) {
  const { notify } = useApp();
  const [discovered, setDiscovered] = useState<DiscoveredDevice[]>([]);
  const [address, setAddress] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [verification, setVerification] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const poll = () => api.listDiscovered().then(setDiscovered, () => {});
    void poll();
    const t = setInterval(poll, 2000);
    return () => clearInterval(t);
  }, []);

  useEngineEvents((e) => {
    if (e.kind === "pairingVerification") setVerification(e.verificationCode);
  });

  async function join() {
    setBusy(true);
    setError(null);
    setVerification(null);
    try {
      const d = await api.pairJoin(address, code);
      notify("ok", `Paired with ${d.name}.`);
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
      title="Join a device"
      onClose={onClose}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button
            variant="primary"
            busy={busy}
            disabled={!address.trim() || code.replace(/[^0-9a-z]/gi, "").length !== 8}
            onClick={join}
          >
            Join
          </Button>
        </>
      }
    >
      <p className="text-sm">
        On the other computer choose <strong>Pair new device</strong>. It shows a code and an address.
      </p>
      {discovered.length > 0 && (
        <fieldset className="space-y-1">
          <legend className="flex items-center gap-1 text-sm font-medium">
            <Radar size={14} />
            Found on your network
          </legend>
          {discovered.map((d) => (
            <label key={d.deviceId} className="flex items-center gap-2 text-sm">
              <input type="radio" name="disc" checked={address === d.address} onChange={() => setAddress(d.address)} />
              {d.name} <span className="font-mono text-xs text-muted">{d.address}</span>
            </label>
          ))}
        </fieldset>
      )}
      <Field label="Address" hint="Shown on the other computer, e.g. 192.168.1.20:47821">
        <input className={`${inputClass} font-mono`} value={address} onChange={(e) => setAddress(e.target.value)} />
      </Field>
      <Field label="Pairing code">
        <input
          className={`${inputClass} font-mono uppercase tracking-widest`}
          maxLength={9}
          value={code}
          onChange={(e) => setCode(e.target.value)}
        />
      </Field>
      {verification && busy && (
        <div className="rounded-lg bg-surface-2 py-3 text-center">
          <p className="text-xs text-muted">Waiting for approval. The other screen should show:</p>
          <p className="font-mono text-2xl font-semibold tracking-widest">{verification}</p>
        </div>
      )}
      {error && <Notice tone="danger">{error}</Notice>}
    </Modal>
  );
}

function RemoteAccess({
  devices,
  onChanged,
  goSettings,
}: {
  devices: DeviceView[];
  onChanged: () => void;
  goSettings: () => void;
}) {
  const { notify } = useApp();
  const [status, setStatus] = useState<RemoteStatus | null>(null);
  const [includeAsk, setIncludeAsk] = useState<{ deviceId: string; ws: string } | null>(null);
  const [health, setHealth] = useState<Record<string, string>>({});

  const load = useCallback(() => api.remoteStatus().then(setStatus, () => {}), []);
  useEffect(() => {
    void load();
  }, [load]);
  useEngineEvents((e) => {
    if (e.kind === "devicesChanged" || e.kind === "peerOnline") void load();
  });

  if (!status || devices.length === 0) return null;
  const act = async (fn: () => Promise<unknown>, ok?: string) => {
    try {
      await fn();
      if (ok) notify("ok", ok);
      await load();
      onChanged();
    } catch (e) {
      notify("danger", errorMessage(e));
    }
  };
  const open = async (deviceId: string, ws: string, allowInclude: boolean) => {
    try {
      await api.remoteOpen(deviceId, ws, allowInclude);
    } catch (e) {
      if (errorCode(e) === "include_needed") setIncludeAsk({ deviceId, ws });
      else notify("danger", errorMessage(e));
    }
  };

  return (
    <Card>
      <CardHeader
        title="Remote access"
        subtitle="Open another computer's projects in VS Code through SSH. Code and Claude Code run on that computer."
      />
      <div className="space-y-4 px-4 py-3 text-sm">
        {!status.remoteSshInstalled && (
          <Notice tone="warn">
            Install the “Remote - SSH” extension (Microsoft) in VS Code on this computer to open remote projects.
          </Notice>
        )}
        <ul className="divide-y divide-line">
          {devices.map((d) => {
            const host = status.hosts.find((h) => h.deviceId === d.id);
            return (
              <li key={d.id} className="py-2">
                <div className="flex flex-wrap items-center gap-2">
                  <Server size={15} className="text-muted" />
                  <span className="flex-1 font-medium">{d.name}</span>
                  {host ? (
                    <>
                      <Button
                        variant="ghost"
                        onClick={async () => {
                          const r = await api.remoteCheck(d.id);
                          setHealth((h) => ({
                            ...h,
                            [d.id]: `DevBridge ${r.devbridgeOnline ? "connected" : "offline"} · SSH ${r.sshReachable ? "reachable" : "not reachable"}`,
                          }));
                        }}
                      >
                        Check connection
                      </Button>
                      <Button variant="ghost" onClick={() => act(() => api.remoteRevoke(d.id), "Remote access removed.")}>
                        Forget
                      </Button>
                    </>
                  ) : (
                    <Button
                      icon={KeyRound}
                      disabled={!d.online}
                      title={d.online ? undefined : "The device must be connected"}
                      onClick={() => act(() => api.remoteRequestAccess(d.id), `Request sent. Approve it on ${d.name}.`)}
                    >
                      Request access
                    </Button>
                  )}
                </div>
                {health[d.id] && <p className="mt-1 text-xs text-muted">{health[d.id]}</p>}
                {host && (
                  <ul className="mt-2 space-y-1 pl-6">
                    {host.workspaces.length === 0 && (
                      <li className="text-xs text-muted">{d.name} hasn't made any project available remotely.</li>
                    )}
                    {host.workspaces.map((w) => (
                      <li key={w.ws} className="flex items-center gap-2">
                        <span className="flex-1">
                          {w.name} <span className="font-mono text-xs text-muted">{w.path}</span>
                        </span>
                        <Button icon={CodeXml} onClick={() => open(d.id, w.ws, false)}>
                          Open remotely
                        </Button>
                      </li>
                    ))}
                  </ul>
                )}
              </li>
            );
          })}
        </ul>
        {status.grants.length > 0 && (
          <div>
            <p className="mb-1 font-medium">Devices that can open this computer's projects</p>
            <ul className="space-y-1">
              {status.grants.map((g) => (
                <li key={g.deviceId} className="flex items-center gap-2">
                  <span className="flex-1">
                    {g.deviceName} <span className="text-xs text-muted">since {relativeTime(g.grantedAt)}</span>
                  </span>
                  <Button
                    variant="ghost"
                    icon={ShieldOff}
                    onClick={() => act(() => api.remoteRevoke(g.deviceId), "Access revoked and SSH key removed.")}
                  >
                    Revoke
                  </Button>
                </li>
              ))}
            </ul>
          </div>
        )}
        <p className="text-xs text-muted">
          Choose which of this computer's projects are reachable in each project's “Remote access” tab.{" "}
          <button type="button" className="underline" onClick={goSettings}>
            Network settings
          </button>
        </p>
      </div>
      {includeAsk && (
        <Modal
          open
          title="Add DevBridge hosts to your SSH config?"
          onClose={() => setIncludeAsk(null)}
          footer={
            <>
              <Button onClick={() => setIncludeAsk(null)}>Cancel</Button>
              <Button
                variant="primary"
                onClick={() => {
                  const a = includeAsk;
                  setIncludeAsk(null);
                  void open(a.deviceId, a.ws, true);
                }}
              >
                Add and open
              </Button>
            </>
          }
        >
          <p className="text-sm">
            VS Code reads SSH hosts from <span className="font-mono">~/.ssh/config</span>. DevBridge keeps its entries in a
            separate file and needs one line at the top of your config:
          </p>
          <pre className="rounded bg-surface-2 p-2 font-mono text-xs">Include devbridge_config</pre>
          <p className="text-xs text-muted">Nothing else in your config changes. A backup is saved as config.devbridge-backup.</p>
        </Modal>
      )}
    </Card>
  );
}

function RenameDialog({ device, onClose, onSaved }: { device: DeviceView; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState(device.name);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  async function save() {
    setBusy(true);
    try {
      await api.renameLocalDevice(name);
      onSaved();
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
      title="Rename this device"
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" busy={busy} disabled={!name.trim()} onClick={save}>
            Save
          </Button>
        </>
      }
    >
      {error && <Notice tone="danger">{error}</Notice>}
      <Field label="Device name" hint="Shown to your other computers.">
        <input className={inputClass} value={name} maxLength={64} onChange={(e) => setName(e.target.value)} />
      </Field>
    </Modal>
  );
}
