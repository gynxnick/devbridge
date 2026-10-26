import { CircleCheck, Circle } from "lucide-react";
import { useApp } from "../lib/app-context";
import { Card, CardHeader } from "./ui";

/** First-run checklist. Disappears once every step is done. */
export function GettingStarted({ onAddProject }: { onAddProject: () => void }) {
  const { overview, navigate } = useApp();
  if (!overview) return null;
  const steps = [
    {
      done: overview.workspaces.length > 0,
      title: "Add a project",
      body: "Import an existing project folder or create a new one.",
      action: onAddProject,
      label: "Add project",
    },
    {
      done: overview.pairedDeviceCount > 0,
      title: "Pair your other computer",
      body: "Install DevBridge there too. On one computer choose Pair new device; on the other choose Join a device.",
      action: () => navigate({ view: "devices" }),
      label: "Open Devices",
    },
    {
      done: overview.workspaces.some((w) => w.syncedDevices > 0),
      title: "Share a project",
      body: "Open a project and share it with your paired device. It joins into a folder you choose.",
      action: () => overview.workspaces[0] && navigate({ view: "project", id: overview.workspaces[0].id }),
      label: "Open project",
    },
  ];
  if (steps.every((s) => s.done)) return null;
  return (
    <Card>
      <CardHeader title="Get started" subtitle="Three steps to continue your work on another computer." />
      <ol className="divide-y divide-line">
        {steps.map((s, i) => (
          <li key={s.title} className="flex items-start gap-3 px-4 py-3 text-sm">
            {s.done ? (
              <CircleCheck size={18} className="mt-0.5 text-ok" aria-label="Done" />
            ) : (
              <Circle size={18} className="mt-0.5 text-muted" aria-label="To do" />
            )}
            <div className="flex-1">
              <p className={`font-medium ${s.done ? "text-muted line-through" : ""}`}>
                {i + 1}. {s.title}
              </p>
              {!s.done && <p className="text-muted">{s.body}</p>}
            </div>
            {!s.done && (
              <button type="button" onClick={s.action} className="text-sm font-medium text-accent hover:underline">
                {s.label}
              </button>
            )}
          </li>
        ))}
      </ol>
      <p className="border-t border-line px-4 py-2 text-xs text-muted">
        Prefer a single copy? A project's <em>Remote access</em> tab lets your other computer work on this one through VS Code
        Remote-SSH instead.
      </p>
    </Card>
  );
}
