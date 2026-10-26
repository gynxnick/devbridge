import { useState } from "react";
import { ShieldCheck } from "lucide-react";
import { api, errorMessage } from "../lib/api";
import { useApp } from "../lib/app-context";
import { Button, Modal, Notice } from "./ui";

/** Pairing approval and remote-access approval, shown wherever the user is. */
export function GlobalPrompts() {
  return (
    <>
      <PairingApproval />
      <RemoteApproval />
    </>
  );
}

function PairingApproval() {
  const { pairingPrompt, clearPairingPrompt, notify, refresh } = useApp();
  const [busy, setBusy] = useState(false);
  if (!pairingPrompt) return null;
  const decide = async (approve: boolean) => {
    setBusy(true);
    try {
      await api.pairingDecide(pairingPrompt.requestId, approve);
      if (approve) notify("ok", `Paired with ${pairingPrompt.deviceName}.`);
      await refresh();
    } catch (e) {
      notify("danger", errorMessage(e));
    } finally {
      setBusy(false);
      clearPairingPrompt();
    }
  };
  return (
    <Modal
      open
      title="Pair with this device?"
      onClose={() => void decide(false)}
      footer={
        <>
          <Button onClick={() => void decide(false)} disabled={busy}>
            Decline
          </Button>
          <Button variant="primary" icon={ShieldCheck} busy={busy} onClick={() => void decide(true)}>
            Approve
          </Button>
        </>
      }
    >
      <p className="text-sm">
        <span className="font-medium">{pairingPrompt.deviceName}</span> entered your pairing code and wants to pair.
      </p>
      <div className="rounded-lg bg-surface-2 py-3 text-center">
        <p className="text-xs text-muted">Verification code</p>
        <p className="font-mono text-2xl font-semibold tracking-widest">{pairingPrompt.verificationCode}</p>
      </div>
      <Notice tone="info">
        Approve only if the other computer shows the same verification code. Paired devices can sync the projects you share with
        them.
      </Notice>
    </Modal>
  );
}

function RemoteApproval() {
  const { remotePrompt, clearRemotePrompt, notify } = useApp();
  const [busy, setBusy] = useState(false);
  if (!remotePrompt) return null;
  const decide = async (allow: boolean) => {
    setBusy(true);
    try {
      await api.remoteDecide(remotePrompt.deviceId, allow);
      if (allow) notify("ok", "Remote access granted.");
    } catch (e) {
      notify("danger", errorMessage(e));
    } finally {
      setBusy(false);
      clearRemotePrompt();
    }
  };
  return (
    <Modal
      open
      title="Allow remote access?"
      onClose={() => void decide(false)}
      footer={
        <>
          <Button onClick={() => void decide(false)} disabled={busy}>
            Decline
          </Button>
          <Button variant="primary" busy={busy} onClick={() => void decide(true)}>
            Allow
          </Button>
        </>
      }
    >
      <p className="text-sm">A paired device asks to open this computer's remote-enabled projects in VS Code over SSH.</p>
      <Notice tone="warn">
        Allowing adds its SSH key to your account's authorized keys. Through SSH it can run commands as you, like any SSH login.
        You can revoke it anytime under Devices → Remote access.
      </Notice>
    </Modal>
  );
}
