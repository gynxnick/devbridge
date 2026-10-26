import { cloneElement, isValidElement, useEffect, useId, useRef, type ButtonHTMLAttributes, type ReactNode } from "react";
import {
  BookOpen,
  CodeXml,
  Database,
  Folder,
  Gamepad2,
  Globe,
  LoaderCircle,
  Package,
  Server,
  Smartphone,
  Terminal,
  X,
  type LucideIcon,
} from "lucide-react";
import type { WorkspaceColor, WorkspaceIcon } from "../lib/types";

export const COLOR_HEX: Record<WorkspaceColor, string> = {
  slate: "#64748b",
  red: "#ef4444",
  orange: "#f97316",
  amber: "#f59e0b",
  green: "#22c55e",
  teal: "#14b8a6",
  blue: "#3b82f6",
  indigo: "#6366f1",
  violet: "#8b5cf6",
  pink: "#ec4899",
};

export const ICONS: Record<WorkspaceIcon, LucideIcon> = {
  folder: Folder,
  code: CodeXml,
  globe: Globe,
  server: Server,
  smartphone: Smartphone,
  gamepad: Gamepad2,
  database: Database,
  package: Package,
  terminal: Terminal,
  book: BookOpen,
};

export function ProjectGlyph({ color, icon, size = 36 }: { color: WorkspaceColor; icon: WorkspaceIcon; size?: number }) {
  const Icon = ICONS[icon];
  return (
    <span
      aria-hidden
      className="inline-flex shrink-0 items-center justify-center rounded-lg"
      style={{ width: size, height: size, background: `${COLOR_HEX[color]}22`, color: COLOR_HEX[color] }}
    >
      <Icon size={size * 0.5} />
    </span>
  );
}

type Variant = "primary" | "secondary" | "ghost" | "danger";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent text-accent-fg hover:opacity-90",
  secondary: "border border-line bg-surface hover:bg-surface-2",
  ghost: "hover:bg-surface-2",
  danger: "bg-danger text-white hover:opacity-90",
};

export function Button({
  variant = "secondary",
  busy = false,
  icon: Icon,
  children,
  className = "",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; busy?: boolean; icon?: LucideIcon }) {
  return (
    <button
      type="button"
      {...rest}
      disabled={rest.disabled || busy}
      className={`inline-flex items-center justify-center gap-2 rounded-md px-3 py-1.5 text-sm font-medium transition disabled:cursor-not-allowed disabled:opacity-50 ${VARIANTS[variant]} ${className}`}
    >
      {busy ? <LoaderCircle size={15} className="animate-spin" /> : Icon ? <Icon size={15} /> : null}
      {children}
    </button>
  );
}

export function Card({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <section className={`rounded-xl border border-line bg-surface ${className}`}>{children}</section>;
}

export function CardHeader({ title, action, subtitle }: { title: string; subtitle?: string; action?: ReactNode }) {
  return (
    <header className="flex items-start justify-between gap-3 border-b border-line px-4 py-3">
      <div>
        <h2 className="text-sm font-semibold">{title}</h2>
        {subtitle && <p className="mt-0.5 text-xs text-muted">{subtitle}</p>}
      </div>
      {action}
    </header>
  );
}

const TONE: Record<string, string> = {
  ok: "text-ok bg-ok/10",
  warn: "text-warn bg-warn/10",
  danger: "text-danger bg-danger/10",
  info: "text-info bg-info/10",
  muted: "text-muted bg-surface-2",
};

export function Badge({ tone, children, title }: { tone: keyof typeof TONE | string; children: ReactNode; title?: string }) {
  return (
    <span
      title={title}
      className={`inline-flex items-center gap-1.5 rounded-full px-2 py-0.5 text-xs font-medium ${TONE[tone] ?? TONE.muted}`}
    >
      {children}
    </span>
  );
}

export function Dot({ tone }: { tone: string }) {
  const c = { ok: "bg-ok", warn: "bg-warn", danger: "bg-danger", info: "bg-info", muted: "bg-muted" }[tone] ?? "bg-muted";
  return <span aria-hidden className={`inline-block h-2 w-2 rounded-full ${c}`} />;
}

export function Notice({ tone = "info", children }: { tone?: "info" | "warn" | "danger"; children: ReactNode }) {
  const c = { info: "border-info/30 bg-info/5", warn: "border-warn/30 bg-warn/5", danger: "border-danger/30 bg-danger/5" }[tone];
  return (
    <div role={tone === "danger" ? "alert" : "note"} className={`rounded-lg border px-3 py-2 text-sm ${c}`}>
      {children}
    </div>
  );
}

export function EmptyState({ icon: Icon, title, children }: { icon: LucideIcon; title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 px-6 py-12 text-center">
      <Icon size={28} className="text-muted" />
      <p className="font-medium">{title}</p>
      {children && <div className="max-w-md text-sm text-muted">{children}</div>}
    </div>
  );
}

/** Accessible modal dialog built on <dialog>. */
export function Modal({
  open,
  title,
  onClose,
  children,
  footer,
}: {
  open: boolean;
  title: string;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) {
      if (typeof d.showModal === "function") d.showModal();
      else d.setAttribute("open", "");
    }
    if (!open && d.open) {
      if (typeof d.close === "function") d.close();
      else d.removeAttribute("open");
    }
  }, [open]);
  return (
    <dialog
      ref={ref}
      aria-labelledby={titleId}
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
      className="m-auto w-[min(560px,92vw)] rounded-xl border border-line bg-surface p-0 text-fg shadow-2xl backdrop:bg-black/40"
    >
      {open && (
        <>
          <header className="flex items-center justify-between border-b border-line px-5 py-3">
            <h2 id={titleId} className="font-semibold">
              {title}
            </h2>
            <button type="button" aria-label="Close" onClick={onClose} className="rounded p-1 hover:bg-surface-2">
              <X size={16} />
            </button>
          </header>
          <div className="space-y-4 px-5 py-4">{children}</div>
          {footer && <footer className="flex justify-end gap-2 border-t border-line px-5 py-3">{footer}</footer>}
        </>
      )}
    </dialog>
  );
}

/** Label + control + hint. The single child control receives the label's id. */
export function Field({ label, hint, children }: { label: string; hint?: ReactNode; children: ReactNode }) {
  const id = useId();
  const control = isValidElement<{ id?: string }>(children) ? cloneElement(children, { id }) : children;
  return (
    <div className="space-y-1">
      <label htmlFor={id} className="block text-sm font-medium">
        {label}
      </label>
      {control}
      {hint && <p className="text-xs text-muted">{hint}</p>}
    </div>
  );
}

export const inputClass = "w-full rounded-md border border-line bg-bg px-2.5 py-1.5 text-sm outline-none focus:border-accent";
