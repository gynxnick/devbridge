import { describe, expect, it } from "vitest";
import { relativeTime, shortHash } from "./format";

describe("relativeTime", () => {
  const now = 1_700_000_000_000;
  it("handles null and recent times", () => {
    expect(relativeTime(null, now)).toBe("Never");
    expect(relativeTime(now - 10_000, now)).toBe("Just now");
    expect(relativeTime(now - 5 * 60_000, now)).toBe("5 min ago");
    expect(relativeTime(now - 3 * 3_600_000, now)).toBe("3 h ago");
    expect(relativeTime(now - 2 * 86_400_000, now)).toBe("2 d ago");
  });
});

describe("shortHash", () => {
  it("abbreviates", () => {
    expect(shortHash("abcdef1234")).toBe("abcdef1");
    expect(shortHash(null)).toBe("—");
  });
});
