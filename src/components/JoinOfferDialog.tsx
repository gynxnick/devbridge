import { useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { FolderOpen, TriangleAlert } from "lucide-react";
import { api, errorMessage } from "../lib/api";
import { useApp } from "../lib/app-context";
import type { PathInspection, WorkspaceOffer } from "../lib/types";
import { Button, Modal, Notice } from "./ui";

/** Accept a project another device shared: choose where it lives here. */
export function JoinOfferDialog({ offer, onClose }: { offer: WorkspaceOffer; onClose: () => void }) {
  const { refresh, notify, navigate, overview } = useApp();
  const alreadyHere = overview?.workspaces.some((w) => w.id === offer.workspaceId) ?? false;
  const [path, setPath] = useState<string | null>(null);
  const [inspection, setInspection] = useState<PathInspection | null>(null);
  const [nonEmpty, setNonEmpty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function choose() {
    const picked = await openDialog({ directory: true, multiple: false, title: `Choose a folder for “${offer.name}”` });
    if (typeof picked !== "string") return;
    setPath(picked);
    setError(null);
    try {
      const i = await api.inspectPath(picked);
      setInspection(i);
      setNonEmpty(!!i.git || i.warnings.length > 0);
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function join() {
    setBusy(true);
    setError(null);
    try {
      await api.joinOffer(offer.workspaceId, offer.deviceId, alreadyHere ? undefined : (inspection?.canonicalPath ?? undefined));
      await refresh();
      notify("ok", `Joined “${offer.name}”. Files will sync from ${offer.deviceName}.`);
      onClose();
      navigate({ view: "project", id: offer.workspaceId });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const ready = alreadyHere || (!!inspection?.canonicalPath && !inspection.error);
  return (
    <Modal
      open
      onClose={onClose}
      title={`Join “${offer.name}”`}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" busy={busy} disabled={!ready} onClick={join}>
            Join and sync
          </Button>
        </>
      }
    >
      <p className="text-sm">
        <span className="font-medium">{offer.deviceName}</span> shared this project with you.
        {alreadyHere
          ? " It's already registered here, so syncing with that device simply starts."
          : " Choose where it should live on this computer."}
      </p>
      {!alreadyHere && (
        <>
          <div className="flex items-center gap-2">
            <Button icon={FolderOpen} onClick={choose}>
              Choose folder…
            </Button>
            <span className="min-w-0 truncate font-mono text-xs text-muted" title={path ?? ""}>
              {path ?? "No folder selected"}
            </span>
          </div>
          <Notice tone="info">
            An empty folder is simplest. If you pick an existing copy (for example your own Git clone), identical files are
            matched and any file that differs becomes a conflict for you to review — nothing is overwritten.
          </Notice>
          <p className="text-xs text-muted">
            Git history isn't synced. If the project uses Git, clone it here too (or sync into your existing clone) and use your
            remote as usual.
          </p>
        </>
      )}
      {inspection?.error && <Notice tone="danger">{inspection.error}</Notice>}
      {nonEmpty &&
        inspection?.warnings
          .filter((w) => !w.includes("isn't a Git repository"))
          .map((w) => (
            <p key={w} className="flex gap-2 text-sm text-warn">
              <TriangleAlert size={15} className="mt-0.5 shrink-0" />
              <span>{w}</span>
            </p>
          ))}
      {error && <Notice tone="danger">{error}</Notice>}
    </Modal>
  );
}
