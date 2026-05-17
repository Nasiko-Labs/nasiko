import type { MetricsSessionRow } from "@/lib/types";

export type SessionStatusFilter = "all" | "ok" | "error";

export function filterSessionRows(
  rows: MetricsSessionRow[],
  options: {
    status?: SessionStatusFilter;
    startMs?: number;
    endMs?: number;
  },
): MetricsSessionRow[] {
  const { status = "all", startMs, endMs } = options;

  return rows.filter((row) => {
    if (status !== "all" && row.status !== status) {
      return false;
    }

    if (startMs == null && endMs == null) {
      return true;
    }

    const ms = row.start_time ? Date.parse(row.start_time) : NaN;
    if (!Number.isFinite(ms)) {
      return status === "all";
    }
    if (startMs != null && ms < startMs) {
      return false;
    }
    if (endMs != null && ms > endMs) {
      return false;
    }
    return true;
  });
}

export function filterNasikoSessionsInRange<
  T extends { start_time?: string | null },
>(sessions: T[], startMs: number, endMs: number): T[] {
  return sessions.filter((session) => {
    const raw = session.start_time;
    if (!raw) {
      return false;
    }
    const ms = Date.parse(raw);
    if (!Number.isFinite(ms)) {
      return false;
    }
    return ms >= startMs && ms <= endMs;
  });
}
