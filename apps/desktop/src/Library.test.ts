import { describe, expect, it } from "vitest";
import { filterLibrary, type LibraryFilter } from "./Library";
import type { Job } from "./types";

describe("library filtering", () => {
  const filter: LibraryFilter = {
    search: "",
    channel: "",
    state: "",
    from: "",
    to: "",
    sort: "newest",
    protectedOnly: false,
  };
  const jobs = [
    {
      id: "a",
      title: "Concert",
      channel: "One",
      state: "completed",
      created_at: "2026-09-01T12:00:00",
      bytes: 100,
      protected: true,
    },
    {
      id: "b",
      title: "Concert rehearsal",
      channel: "Two",
      state: "partial",
      created_at: "2026-09-02T23:59:59.999",
      bytes: 300,
    },
    {
      id: "c",
      title: "Concert",
      channel: "One",
      state: "recording",
      created_at: "2026-09-03T00:00:00",
      bytes: 500,
    },
  ] as Job[];
  it("combines filters and includes the entire local end date", () => {
    expect(
      filterLibrary(jobs, {
        ...filter,
        search: "CONCERT",
        from: "2026-09-02",
        to: "2026-09-02",
      }).map((j) => j.id),
    ).toEqual(["b"]);
    expect(
      filterLibrary(jobs, {
        ...filter,
        channel: "One",
        protectedOnly: true,
      }).map((j) => j.id),
    ).toEqual(["a"]);
    expect(filterLibrary(jobs, { ...filter, state: "failed" })).toEqual([]);
  });
  it("sorts without mutating the snapshot and always excludes active jobs", () => {
    expect(
      filterLibrary(jobs, { ...filter, sort: "size" }).map((j) => j.id),
    ).toEqual(["b", "a"]);
    expect(
      filterLibrary(jobs, { ...filter, sort: "oldest" }).map((j) => j.id),
    ).toEqual(["a", "b"]);
    expect(jobs.map((j) => j.id)).toEqual(["a", "b", "c"]);
  });
});
