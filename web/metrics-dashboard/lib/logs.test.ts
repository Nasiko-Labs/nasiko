import { describe, expect, it } from "vitest";
import {
  countByLevel,
  filterLogs,
  rowsFromRegistry,
  rowsFromSession,
  synthesizeLogs,
  WARNING_LATENCY_MS,
} from "@/lib/logs";
import type {
  NasikoObservabilitySession,
  NasikoSimpleUserAgent,
  SyntheticLogRow,
} from "@/lib/types";

describe("rowsFromSession", () => {
  it("emits start + end rows for a normal session", () => {
    const session: NasikoObservabilitySession = {
      agent_id: "a2a-translator",
      session_id: "sess-1",
      start_time: "2026-05-17T10:00:00.000Z",
      end_time: "2026-05-17T10:00:02.000Z",
      num_traces: 1,
      trace_latency_ms_p50: 1500,
      last_output: { value: "Bonjour, comment allez-vous?" },
    };
    const rows = rowsFromSession(session);
    expect(rows).toHaveLength(2);
    expect(rows[0].level).toBe("INFO");
    expect(rows[0].message).toMatch(/Request received/);
    expect(rows[1].level).toBe("INFO");
    expect(rows[1].message).toMatch(/Completed/);
    expect(rows[1].latency_ms).toBe(1500);
  });

  it("downgrades the end row to WARNING when p50 latency exceeds threshold", () => {
    const session: NasikoObservabilitySession = {
      agent_id: "a2a-translator",
      session_id: "sess-slow",
      start_time: "2026-05-17T10:00:00.000Z",
      end_time: "2026-05-17T10:00:08.000Z",
      num_traces: 1,
      trace_latency_ms_p50: WARNING_LATENCY_MS + 1,
      last_output: { value: "done" },
    };
    const rows = rowsFromSession(session);
    const endRow = rows[1];
    expect(endRow.level).toBe("WARNING");
    expect(endRow.message).toMatch(/Slow response/);
  });

  it("emits an ERROR end row when the session looks errored", () => {
    const session: NasikoObservabilitySession = {
      agent_id: "a2a-translator",
      session_id: "sess-err",
      start_time: "2026-05-17T10:00:00.000Z",
      end_time: "2026-05-17T10:00:01.000Z",
      num_traces: 1,
      trace_latency_ms_p50: 800,
      last_output: { value: "Error code: 500 - service unavailable" },
    };
    const rows = rowsFromSession(session);
    expect(rows[1].level).toBe("ERROR");
  });
});

describe("rowsFromRegistry", () => {
  it("emits one INFO row per agent at the snapshot time", () => {
    const agents: NasikoSimpleUserAgent[] = [
      { agent_id: "a2a-translator", name: "Translator Agent" },
      { agent_id: "a2a-router", name: "Router Agent" },
    ];
    const rows = rowsFromRegistry(agents, "2026-05-17T12:00:00.000Z");
    expect(rows).toHaveLength(2);
    expect(rows[0].level).toBe("INFO");
    expect(rows[0].service).toBe("registry");
    expect(rows[0].message).toMatch(/Translator Agent/);
  });
});

describe("synthesizeLogs", () => {
  it("sorts newest first and combines sources", () => {
    const sessions: NasikoObservabilitySession[] = [
      {
        agent_id: "a2a-translator",
        session_id: "old",
        start_time: "2026-05-17T08:00:00.000Z",
        end_time: "2026-05-17T08:00:01.000Z",
        last_output: { value: "ok" },
      },
      {
        agent_id: "a2a-translator",
        session_id: "new",
        start_time: "2026-05-17T11:00:00.000Z",
        end_time: "2026-05-17T11:00:01.000Z",
        last_output: { value: "ok" },
      },
    ];
    const agents: NasikoSimpleUserAgent[] = [
      { agent_id: "a2a-translator", name: "Translator Agent" },
    ];
    const { logs, source_counts } = synthesizeLogs({
      sessions,
      agents,
      generatedAt: new Date("2026-05-17T12:00:00.000Z"),
    });

    expect(source_counts.sessions).toBe(4);
    expect(source_counts.registry).toBe(1);
    expect(logs[0].ts >= logs[logs.length - 1].ts).toBe(true);
    expect(logs[0].service).toBe("registry"); // snapshot at 12:00 is newest
  });

  it("respects maxRows cap", () => {
    const sessions: NasikoObservabilitySession[] = Array.from({ length: 50 }, (_, i) => ({
      agent_id: "a2a-translator",
      session_id: `s-${i}`,
      start_time: `2026-05-17T10:${String(i % 60).padStart(2, "0")}:00.000Z`,
      last_output: { value: "ok" },
    }));
    const { logs } = synthesizeLogs({
      sessions,
      agents: [],
      generatedAt: new Date("2026-05-17T12:00:00.000Z"),
      maxRows: 10,
    });
    expect(logs).toHaveLength(10);
  });
});

describe("filterLogs", () => {
  const rows: SyntheticLogRow[] = [
    {
      id: "1",
      ts: "2026-05-17T10:00:00.000Z",
      level: "INFO",
      service: "agent:a2a-translator",
      message: "Request received by a2a-translator",
    },
    {
      id: "2",
      ts: "2026-05-17T10:00:01.000Z",
      level: "ERROR",
      service: "agent:a2a-translator",
      message: "Request failed — timeout",
    },
    {
      id: "3",
      ts: "2026-05-17T10:00:02.000Z",
      level: "WARNING",
      service: "agent:a2a-router",
      message: "Slow response (6s)",
    },
  ];

  it("filters by level set", () => {
    const out = filterLogs(rows, { levels: new Set(["ERROR"]) });
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe("2");
  });

  it("filters by free-text search across message/service/agent", () => {
    const out = filterLogs(rows, { search: "router" });
    expect(out).toHaveLength(1);
    expect(out[0].service).toBe("agent:a2a-router");
  });

  it("counts by level", () => {
    expect(countByLevel(rows)).toEqual({ INFO: 1, WARNING: 1, ERROR: 1 });
  });
});
