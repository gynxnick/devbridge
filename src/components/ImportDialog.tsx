import { useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { FolderOpen, GitBranch, TriangleAlert } from "lucide-react";
import { api, errorMessage } from "../lib/api";
import { useApp } from "../lib/app-context";
import { WORKSPACE_COLORS, WORKSPACE_ICONS, type PathInspection, type WorkspaceColor, type WorkspaceIcon } from "../lib/types";
import { Button, COLOR_HEX, Field, ICONS, Modal, Notice, inputClass } from "./ui";

export type ImportMode = "import" | "create";

/** Mount only while open: each opening starts with a fresh form. */
export function ImportDialog({ mode, onClose }: { mode: ImportMode; onClose: () => void }) {
  const { refresh, navigate, notify } = useApp();
  const [path, setPath] = useState("");
  const [inspection, setInspection] = useState<PathInspection | null>(null);
  const [name, setName] = useState("");
  const [color, setColor] = useState<WorkspaceColor>("blue");
  const [icon, setIcon] = useState<WorkspaceIcon>("code");
  const [initGit, setInitGit] = useState(mode === "create");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function choose() {
    const picked = await openDialog({
      directory: true,
      multiple: false,
      title: mode === "create" ? "Choose or create an empty folder for the new project" : "Choose a project folder",
    });
    if (typeof picked !== "string") return;
    setPath(picked);
    setError(null);
    try {
      const i = await api.inspectPath(picked);
      setInspection(i);
      setName(i.suggestedName);
      setColor(i.suggestedColor);
      setIcon(i.git ? "code" : "folder");
    } catch (e) {
      setError(errorMessage(e));
    }
  }

  async function submit() {
    if (!inspection?.canonicalPath) return;
    setBusy(true);
    setError(null);
    try {
      const ws = await api.importWorkspace({
        path: inspection.canonicalPath,
        name,
        color,
        icon,
        initGit: initGit && !inspection.git,
      });
      await refresh();
      notify("ok", `Added “${ws.name}”.`);
      onClose();
      navigate({ view: "project", id: ws.id });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  const canSubmit = !!inspection?.canonicalPath && !inspection.error && name.trim().length > 0;

  return (
    <Modal
      open
      onClose={onClose}
      title={mode === "create" ? "Create project" : "Import project"}
      footer={
        <>
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" onClick={submit} disabled={!canSubmit} busy={busy}>
            {mode === "create" ? "Create project" : "Add project"}
          </Button>
        </>
      }
    >
      <p className="text-sm text-muted">
        {mode === "create"
          ? "Pick a folder (you can create a new one in the picker). DevBridge registers it and can initialize a Git repository."
          : "Pick an existing project folder. DevBridge only registers it; your files aren't moved or changed."}
      </p>
      <div className="flex items-center gap-2">
        <Button icon={FolderOpen} onClick={choose}>
          Choose folder…
        </Button>
        <span className="min-w-0 truncate font-mono text-xs text-muted" title={path}>
          {path || "No folder selected"}
        </span>
      </div>

      {inspection?.error && <Notice tone="danger">{inspection.error}</Notice>}
      {error && <Notice tone="danger">{error}</Notice>}

      {inspection && !inspection.error && (
        <>
          {inspection.git ? (
            <p className="flex items-center gap-2 text-sm">
              <GitBranch size={15} className="text-ok" />
              Git repository{inspection.git.branch ? ` on branch ${inspection.git.branch}` : ""}
            </p>
          ) : (
            inspection.gitAvailable && (
              <label className="flex items-center gap-2 text-sm">
                <input type="checkbox" checked={initGit} onChange={(e) => setInitGit(e.target.checked)} />
                Initialize a Git repository in this folder
              </label>
            )
          )}
          {inspection.warnings
            .filter((w) => !(initGit && w.includes("isn't a Git repository")))
            .map((w) => (
              <p key={w} className="flex gap-2 text-sm text-warn">
                <TriangleAlert size={15} className="mt-0.5 shrink-0" />
                <span>{w}</span>
              </p>
            ))}
          <Field label="Name">
            <input className={inputClass} value={name} maxLength={80} onChange={(e) => setName(e.target.value)} />
          </Field>
          <fieldset>
            <legend className="mb-1 text-sm font-medium">Color</legend>
            <div className="flex flex-wrap gap-1.5">
              {WORKSPACE_COLORS.map((c) => (
                <button
                  key={c}
                  type="button"
                  aria-label={c}
                  aria-pressed={c === color}
                  onClick={() => setColor(c)}
                  className={`h-6 w-6 rounded-full ring-offset-2 ring-offset-surface ${c === color ? "ring-2 ring-fg" : ""}`}
                  style={{ background: COLOR_HEX[c] }}
                />
              ))}
            </div>
          </fieldset>
          <fieldset>
            <legend className="mb-1 text-sm font-medium">Icon</legend>
            <div className="flex flex-wrap gap-1.5">
              {WORKSPACE_ICONS.map((i) => {
                const Icon = ICONS[i];
                return (
                  <button
                    key={i}
                    type="button"
                    aria-label={i}
                    aria-pressed={i === icon}
                    onClick={() => setIcon(i)}
                    className={`rounded-md border p-1.5 ${i === icon ? "border-accent text-accent" : "border-line text-muted hover:text-fg"}`}
                  >
                    <Icon size={16} />
                  </button>
                );
              })}
            </div>
          </fieldset>
        </>
      )}
    </Modal>
  );
}
