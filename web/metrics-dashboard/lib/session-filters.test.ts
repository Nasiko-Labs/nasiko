import { describe, expect, it } from "vitest";
import { filterSessionRows } from "@/lib/session-filters";
import type { MetricsSessionRow } from "@/lib/types";

const rows: MetricsSessionRow[] = [
  {
    session_id: "a",
    start_time: "2026-05-17T10:00:00.000Z",
    num_traces: 1,
    latency_ms_p50: 100,
    status: "ok",
    preview: "ok",
  },
  {
    session_id: "b",
    start_time: "2026-05-17T11:00:00.000Z",
    num_traces: 1,
    latency_ms_p50: 200,
    status: "error",
    preview: "failed",
  },
];

describe("filterSessionRows", () => {
  it("filters by status", () => {
    expect(filterSessionRows(rows, { status: "error" })).toHaveLength(1);
    expect(filterSessionRows(rows, { status: "error" })[0].session_id).toBe("b");
  });

  it("filters by time range", () => {
    const startMs = Date.parse("2026-05-17T10:30:00.000Z");
    const endMs = Date.parse("2026-05-17T10:30:00.000Z");
    expect(filterSessionRows(rows, { startMs, endMs })).toHaveLength(0);
  });
});
