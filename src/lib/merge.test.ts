import { describe, expect, it } from "vitest";
import { hasMarkers, sideBySide, withMarkers } from "./merge";

describe("merge helpers", () => {
  it("builds side-by-side rows", () => {
    const rows = sideBySide("a\nb\nc\n", "a\nB\nc\nd\n");
    expect(rows).toEqual([
      { left: "a", right: "a", kind: "same" },
      { left: "b", right: "B", kind: "changed" },
      { left: "c", right: "c", kind: "same" },
      { left: null, right: "d", kind: "right" },
    ]);
  });

  it("wraps only differing hunks in markers", () => {
    const m = withMarkers("x\nmine\ny\n", "x\ntheirs\ny\n", "Mac", "PC");
    expect(m).toBe("x\n<<<<<<< Mac\nmine\n=======\ntheirs\n>>>>>>> PC\ny\n");
    expect(hasMarkers(m)).toBe(true);
    expect(hasMarkers("x\nmerged\ny\n")).toBe(false);
  });
});
