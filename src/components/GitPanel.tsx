import { useCallback, useEffect, useRef, useState } from "react";
import {
  ArrowDown,
  ArrowUp,
  Download,
  GitBranch,
  GitCommitHorizontal,
  Plus,
  RefreshCw,
  Trash,
  TriangleAlert,
  Upload,
} from "lucide-react";
import { api, errorCode, errorMessage } from "../lib/api";
import { useApp, useEngineEvents } from "../lib/app-context";
import { relativeTime } from "../lib/format";
import type { GitOverview, GitStatusEntry } from "../lib/types";
import { Badge, Button, Card, CardHeader, EmptyState, Field, Modal, Notice, inputClass } from "./ui";

type Pending = { message: string; run: () => Promise<unknown> } | null;

export function GitPanel({ id }: { id: string }) {
  const { notify, refresh: refreshOverview } = useApp();
  const [data, setData] = useState<GitOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [message, setMessage] = useState("");
  const [confirm, setConfirm] = useState<Pending>(null);
  const [newBranch, setNewBranch] = useState<string | null>(null);
  const [remoteForm, setRemoteForm] = useState<{ name: string; url: string } | null>(null);
  const [removeRemote, setRemoveRemote] = useState<string | null>(null);
  const [catchUp, setCatchUp] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await api.gitOverview(id));
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [id]);

  useEffect(() => {
    let live = true;
    api.gitOverview(id).then(
      (d) => live && setData(d),
      (e) => live && setError(errorMessage(e)),
    );
    return () => {
      live = false;
    };
  }, [id]);

  useEngineEvents((e) => {
    if (e.kind === "workspaceChanged" && e.workspaceId === id) {
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => void load(), 600);
    }
  });

  /** Runs a Git action; offers a confirmation step when Git safety requires one. */
  async function run(key: string, fn: () => Promise<unknown>, ok?: string, retryConfirmed?: () => Promise<unknown>) {
    setBusy(key);
    try {
      await fn();
      if (ok) notify("ok", ok);
      await load();
      void refreshOverview();
    } catch (e) {
      if (errorCode(e) === "needs_confirmation" && retryConfirmed) {
        setConfirm({ message: errorMessage(e), run: retryConfirmed });
      } else {
        notify("danger", errorMessage(e));
      }
    } finally {
      setBusy(null);
    }
  }

  if (error) return <Notice tone="danger">{error}</Notice>;
  if (!data) return null;
  if (!data.isRepo) {
    return (
      <Card>
        <EmptyState icon={GitBranch} title="Not a Git repository">
          <p>
            Git keeps your history; DevBridge syncs working files. Initialize a repository here or clone your project into this
            folder.
          </p>
          <div className="mt-3">
            <Button busy={busy === "init"} onClick={() => run("init", () => api.gitInit(id), "Repository initialized.")}>
              Initialize Git
            </Button>
          </div>
        </EmptyState>
      </Card>
    );
  }
  const st = data.status!;
  const entries = st.entries.filter((e) => e.kind !== "ignored");
  const staged = entries.filter((e) => e.kind !== "untracked" && e.kind !== "unmerged" && e.index !== ".");
  const changed = entries.filter((e) => e.kind === "unmerged" || e.kind === "untracked" || e.worktree !== ".");
  const toggle = (e: GitStatusEntry, stage: boolean) =>
    run(`file:${e.path}`, () => (stage ? api.gitStage(id, [e.path]) : api.gitUnstage(id, [e.path])));

  return (
    <div className="space-y-6">
      <Card>
        <CardHeader
          title="Repository"
          subtitle={data.toplevel ?? undefined}
          action={
            <Button variant="ghost" icon={RefreshCw} onClick={() => void load()} aria-label="Refresh Git status">
              Refresh
            </Button>
          }
        />
        <div className="flex flex-wrap items-center gap-3 px-4 py-3 text-sm">
          <label className="flex items-center gap-2">
            <GitBranch size={15} />
            <select
              aria-label="Branch"
              className={`${inputClass} !w-auto`}
              value={st.branch ?? ""}
              onChange={(e) => {
                const b = e.target.value;
                if (b === "__new") return setNewBranch("");
                void run(
                  "switch",
                  () => api.gitSwitch(id, b, false, false),
                  `Switched to ${b}.`,
                  () => api.gitSwitch(id, b, false, true),
                );
              }}
            >
              {!st.branch && <option value="">Detached HEAD</option>}
              {data.branches.map((b) => (
                <option key={b.name} value={b.name}>
                  {b.name}
                </option>
              ))}
              <option value="__new">New branch…</option>
            </select>
          </label>
          {st.upstream ? <span className="text-muted">tracks {st.upstream}</span> : <Badge tone="warn">No upstream</Badge>}
          {st.ahead > 0 && (
            <Badge tone="info">
              <ArrowUp size={12} />
              {st.ahead} to push
            </Badge>
          )}
          {st.behind > 0 && (
            <Badge tone="info">
              <ArrowDown size={12} />
              {st.behind} to pull
            </Badge>
          )}
          <div className="ml-auto flex gap-2">
            <Button icon={Download} busy={busy === "fetch"} onClick={() => run("fetch", () => api.gitFetch(id), "Fetched.")}>
              Fetch
            </Button>
            <Button
              busy={busy === "pull"}
              disabled={!st.upstream}
              onClick={() =>
                run(
                  "pull",
                  () => api.gitPull(id, false),
                  "Pulled.",
                  () => api.gitPull(id, true),
                )
              }
            >
              Pull
            </Button>
            <Button icon={Upload} busy={busy === "push"} onClick={() => run("push", () => api.gitPush(id), "Pushed.")}>
              Push
            </Button>
          </div>
        </div>
        {st.behind > 0 && entries.length > 0 && (
          <div className="px-4 pb-3">
            <Notice tone="info">
              This branch is {st.behind} commit(s) behind its upstream and has local changes. If those changes are files DevBridge
              synced from your other computer (which already committed them), <strong>Catch up Git</strong> moves this checkout to
              the upstream commit without changing any file — it only proceeds when every file already matches exactly.
              <div className="mt-2">
                <Button onClick={() => setCatchUp(true)}>Catch up Git…</Button>
              </div>
            </Notice>
          </div>
        )}
        {data.autocrlf && data.autocrlf !== "false" && (
          <p className="flex gap-2 px-4 pb-3 text-xs text-warn">
            <TriangleAlert size={14} className="shrink-0" />
            core.autocrlf is “{data.autocrlf}” here. If your other computer uses a different setting, Git may show every file as
            changed; a .gitattributes file with “* text=auto eol=lf” keeps line endings consistent.
          </p>
        )}
      </Card>

      <div className="grid gap-6 lg:grid-cols-2">
        <Card>
          <CardHeader title={`Changes (${entries.length})`} subtitle="Check a file to stage it for the next commit." />
          {entries.length === 0 ? (
            <p className="px-4 py-3 text-sm text-muted">Working tree clean.</p>
          ) : (
            <ul className="max-h-80 divide-y divide-line overflow-y-auto">
              {[...staged, ...changed.filter((c) => !staged.includes(c))].map((e) => {
                const isStaged = staged.includes(e);
                return (
                  <li key={e.path} className="flex items-center gap-2 px-4 py-1.5 text-sm">
                    <input
                      type="checkbox"
                      aria-label={`Stage ${e.path}`}
                      checked={isStaged}
                      disabled={busy?.startsWith("file:") || e.kind === "unmerged"}
                      onChange={(ev) => void toggle(e, ev.target.checked)}
                    />
                    <span className="w-6 font-mono text-xs text-muted">
                      {e.kind === "untracked" ? "??" : `${e.index}${e.worktree}`}
                    </span>
                    <span className="min-w-0 flex-1 truncate font-mono text-xs" title={e.path}>
                      {e.origPath ? `${e.origPath} → ` : ""}
                      {e.path}
                    </span>
                    {e.kind === "unmerged" && <Badge tone="danger">merge conflict</Badge>}
                  </li>
                );
              })}
            </ul>
          )}
          <div className="space-y-2 border-t border-line px-4 py-3">
            <textarea
              aria-label="Commit message"
              rows={2}
              placeholder="Commit message"
              className={inputClass}
              value={message}
              onChange={(e) => setMessage(e.target.value)}
            />
            <div className="flex gap-2">
              <Button
                variant="ghost"
                disabled={changed.length === 0}
                onClick={() =>
                  run("stageall", () =>
                    api.gitStage(
                      id,
                      changed.filter((c) => c.kind !== "unmerged").map((c) => c.path),
                    ),
                  )
                }
              >
                Stage all
              </Button>
              <Button
                variant="primary"
                icon={GitCommitHorizontal}
                className="ml-auto"
                busy={busy === "commit"}
                disabled={staged.length === 0 || !message.trim()}
                onClick={() =>
                  run(
                    "commit",
                    async () => {
                      await api.gitCommit(id, message);
                      setMessage("");
                    },
                    "Committed.",
                  )
                }
              >
                Commit {staged.length > 0 ? `(${staged.length})` : ""}
              </Button>
            </div>
          </div>
        </Card>

        <div className="space-y-6">
          <Card>
            <CardHeader title="Recent commits" />
            {data.log.length === 0 ? (
              <p className="px-4 py-3 text-sm text-muted">No commits yet.</p>
            ) : (
              <ul className="max-h-64 divide-y divide-line overflow-y-auto">
                {data.log.map((c) => (
                  <li key={c.hash} className="flex items-baseline gap-2 px-4 py-1.5 text-sm">
                    <span className="font-mono text-xs text-muted">{c.short}</span>
                    <span className="min-w-0 flex-1 truncate">{c.subject}</span>
                    <span className="shrink-0 text-xs text-muted">{relativeTime(c.time * 1000)}</span>
                  </li>
                ))}
              </ul>
            )}
          </Card>
          <Card>
            <CardHeader
              title="Remotes"
              subtitle="Where history goes. Any Git host works."
              action={
                <Button
                  variant="ghost"
                  icon={Plus}
                  onClick={() => setRemoteForm({ name: data.remotes.length ? "" : "origin", url: "" })}
                >
                  Add
                </Button>
              }
            />
            {data.remotes.length === 0 ? (
              <p className="px-4 py-3 text-sm text-muted">No remotes configured.</p>
            ) : (
              <ul className="divide-y divide-line">
                {data.remotes.map((r) => (
                  <li key={r.name} className="flex items-center gap-2 px-4 py-1.5 text-sm">
                    <span className="font-medium">{r.name}</span>
                    <span className="min-w-0 flex-1 truncate font-mono text-xs text-muted" title={r.url}>
                      {r.url}
                    </span>
                    <Button variant="ghost" onClick={() => setRemoteForm({ name: r.name, url: "" })}>
                      Edit URL
                    </Button>
                    <button
                      type="button"
                      aria-label={`Remove ${r.name}`}
                      onClick={() => setRemoveRemote(r.name)}
                      className="rounded p-1 text-muted hover:text-danger"
                    >
                      <Trash size={14} />
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </Card>
        </div>
      </div>

      {confirm && (
        <Modal
          open
          title="Continue?"
          onClose={() => setConfirm(null)}
          footer={
            <>
              <Button onClick={() => setConfirm(null)}>Cancel</Button>
              <Button
                variant="primary"
                onClick={() => {
                  const c = confirm;
                  setConfirm(null);
                  void run("confirmed", c.run, "Done.");
                }}
              >
                Continue
              </Button>
            </>
          }
        >
          <p className="text-sm">{confirm.message}</p>
          <p className="text-xs text-muted">Git never overwrites uncommitted changes in this operation; it stops instead.</p>
        </Modal>
      )}
      {newBranch !== null && (
        <Modal
          open
          title="New branch"
          onClose={() => setNewBranch(null)}
          footer={
            <>
              <Button onClick={() => setNewBranch(null)}>Cancel</Button>
              <Button
                variant="primary"
                disabled={!newBranch.trim()}
                onClick={() => {
                  const b = newBranch.trim();
                  setNewBranch(null);
                  void run(
                    "switch",
                    () => api.gitSwitch(id, b, true, false),
                    `Created ${b}.`,
                    () => api.gitSwitch(id, b, true, true),
                  );
                }}
              >
                Create and switch
              </Button>
            </>
          }
        >
          <Field label="Branch name" hint="Your uncommitted changes come along to the new branch.">
            <input className={inputClass} value={newBranch} onChange={(e) => setNewBranch(e.target.value)} />
          </Field>
        </Modal>
      )}
      {remoteForm && (
        <Modal
          open
          title={data.remotes.some((r) => r.name === remoteForm.name) ? `Change URL of ${remoteForm.name}` : "Add remote"}
          onClose={() => setRemoteForm(null)}
          footer={
            <>
              <Button onClick={() => setRemoteForm(null)}>Cancel</Button>
              <Button
                variant="primary"
                disabled={!remoteForm.name.trim() || !remoteForm.url.trim()}
                onClick={() => {
                  const f = remoteForm;
                  setRemoteForm(null);
                  const exists = data.remotes.some((r) => r.name === f.name);
                  void run(
                    "remote",
                    () => (exists ? api.gitRemoteSetUrl(id, f.name, f.url) : api.gitRemoteAdd(id, f.name, f.url)),
                    "Remote saved.",
                  );
                }}
              >
                Save
              </Button>
            </>
          }
        >
          <Field label="Name">
            <input
              className={inputClass}
              value={remoteForm.name}
              onChange={(e) => setRemoteForm({ ...remoteForm, name: e.target.value })}
            />
          </Field>
          <Field
            label="URL"
            hint="For example git@github.com:you/project.git. Credentials come from your Git credential helper or SSH keys."
          >
            <input
              className={`${inputClass} font-mono`}
              value={remoteForm.url}
              onChange={(e) => setRemoteForm({ ...remoteForm, url: e.target.value })}
            />
          </Field>
        </Modal>
      )}
      {removeRemote && (
        <Modal
          open
          title={`Remove remote “${removeRemote}”?`}
          onClose={() => setRemoveRemote(null)}
          footer={
            <>
              <Button onClick={() => setRemoveRemote(null)}>Cancel</Button>
              <Button
                variant="danger"
                onClick={() => {
                  const r = removeRemote;
                  setRemoveRemote(null);
                  void run("remote", () => api.gitRemoteRemove(id, r), "Remote removed.");
                }}
              >
                Remove
              </Button>
            </>
          }
        >
          <p className="text-sm">
            This removes the remote and its remote-tracking branches from this checkout. Your own branches and commits are not
            affected.
          </p>
        </Modal>
      )}
      {catchUp && (
        <Modal
          open
          title="Catch up Git with the upstream?"
          onClose={() => setCatchUp(false)}
          footer={
            <>
              <Button onClick={() => setCatchUp(false)}>Cancel</Button>
              <Button
                variant="primary"
                onClick={() => {
                  setCatchUp(false);
                  void run("catchup", async () => {
                    const r = await api.gitCatchUp(id);
                    notify("ok", `Moved to ${r.to.slice(0, 7)} (${r.files} file(s) already matched).`);
                  });
                }}
              >
                Catch up
              </Button>
            </>
          }
        >
          <p className="text-sm">
            DevBridge will check that every file changed by the upstream commits already matches them exactly, then fast-forward
            this branch.
          </p>
          <ul className="list-disc pl-5 text-sm text-muted">
            <li>No file content changes.</li>
            <li>If anything differs, nothing happens and you'll see which file.</li>
            <li>Only the branch and Git's index move; your files are never written.</li>
          </ul>
        </Modal>
      )}
    </div>
  );
}
