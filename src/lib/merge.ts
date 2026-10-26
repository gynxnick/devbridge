import { diffLines } from "diff";

export interface Row {
  left: string | null;
  right: string | null;
  kind: "same" | "changed" | "left" | "right";
}

/** Side-by-side rows for a line diff of `a` (left) and `b` (right). */
export function sideBySide(a: string, b: string): Row[] {
  const rows: Row[] = [];
  const parts = diffLines(a, b);
  const lines = (s: string) => {
    const l = s.split("\n");
    if (l[l.length - 1] === "") l.pop();
    return l;
  };
  for (let i = 0; i < parts.length; i++) {
    const p = parts[i];
    if (!p.added && !p.removed) {
      for (const l of lines(p.value)) rows.push({ left: l, right: l, kind: "same" });
    } else if (p.removed && parts[i + 1]?.added) {
      const l = lines(p.value);
      const r = lines(parts[i + 1].value);
      for (let j = 0; j < Math.max(l.length, r.length); j++) {
        rows.push({ left: l[j] ?? null, right: r[j] ?? null, kind: "changed" });
      }
      i++;
    } else if (p.removed) {
      for (const l of lines(p.value)) rows.push({ left: l, right: null, kind: "left" });
    } else {
      for (const l of lines(p.value)) rows.push({ left: null, right: l, kind: "right" });
    }
  }
  return rows;
}

/** A merge starting point: shared lines once, each differing hunk wrapped in
 * Git-style conflict markers for the user to edit. */
export function withMarkers(local: string, remote: string, localLabel: string, remoteLabel: string): string {
  const out: string[] = [];
  const parts = diffLines(local, remote);
  for (let i = 0; i < parts.length; i++) {
    const p = parts[i];
    if (!p.added && !p.removed) {
      out.push(p.value);
      continue;
    }
    let mine = "";
    let theirs = "";
    if (p.removed) {
      mine = p.value;
      if (parts[i + 1]?.added) {
        theirs = parts[i + 1].value;
        i++;
      }
    } else {
      theirs = p.value;
    }
    const nl = (s: string) => (s === "" || s.endsWith("\n") ? s : s + "\n");
    out.push(`<<<<<<< ${localLabel}\n${nl(mine)}=======\n${nl(theirs)}>>>>>>> ${remoteLabel}\n`);
  }
  return out.join("");
}

export function hasMarkers(text: string): boolean {
  return /^(<{7}|={7}|>{7})( |$)/m.test(text);
}
