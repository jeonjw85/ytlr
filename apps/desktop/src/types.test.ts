import { describe, expect, it } from "vitest";
import { bytes, duration, elapsed, type Job } from "./types";

describe("recording status presentation", () => {
  it("does not display unknown disk capacity as zero", () => {
    expect(bytes(null)).toBe("확인 중");
    expect(bytes(0)).toBe("0 B");
    expect(bytes(1024 ** 3)).toBe("1.0 GiB");
  });
  it("handles multi-day recordings without wrapping hours", () => {
    expect(duration(90061)).toBe("25:01:01");
    expect(duration(-10)).toBe("00:00:00");
  });
  it("shows media duration for finished jobs rather than wall-clock time", () => {
    expect(
      elapsed({
        state: "partial",
        media_seconds: 30,
        started_at: "2020-01-01",
      } as Job),
    ).toBe("00:00:30");
  });
});
