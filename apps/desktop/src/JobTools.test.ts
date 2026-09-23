import { afterEach, describe, expect, it, vi } from "vitest";
import { deadlineDraft, resolveDeadline } from "./JobTools";

describe("stop timer input", () => {
  afterEach(() => vi.useRealTimers());
  it("resolves relative timers once and allows explicit cancellation", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-01-01T12:00:00Z"));
    const draft = deadlineDraft();
    expect(resolveDeadline(draft)).toBeNull();
    expect(resolveDeadline({ ...draft, mode: "minutes", minutes: "120" })).toBe(
      "2026-01-01T14:00:00.000Z",
    );
    expect(() =>
      resolveDeadline({ ...draft, mode: "minutes", minutes: "0" }),
    ).toThrow();
    expect(() =>
      resolveDeadline({ ...draft, mode: "minutes", minutes: "1.5" }),
    ).toThrow();
    expect(() =>
      resolveDeadline({ ...draft, mode: "at", at: "invalid" }),
    ).toThrow();
    expect(resolveDeadline(deadlineDraft("2026-01-01T16:00:00Z"))).toBe(
      "2026-01-01T16:00:00.000Z",
    );
  });
});
