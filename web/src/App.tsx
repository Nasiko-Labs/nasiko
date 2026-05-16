import { useCallback, useEffect, useMemo, useState } from "react";
import { CommandBar } from "./components/CommandBar";
import { LogStream } from "./components/LogStream";
import { TopBar } from "./components/TopBar";
import { platformLogs } from "./data/platformLogs";
import type { LevelFilter, PlatformLog, PlatformLogsResponse, TimeWindow } from "./types";
import { filterLogs, getLogStats, getServices, normalizeApiLogs } from "./utils/logs";

export function App() {
  const [logs, setLogs] = useState<PlatformLog[]>(platformLogs);
  const [dataSource, setDataSource] = useState<"live" | "sample">("sample");
  const [lastUpdated, setLastUpdated] = useState<Date | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [autoRefresh, setAutoRefresh] = useState(true);
  const [query, setQuery] = useState("");
  const [selectedLevel, setSelectedLevel] = useState<LevelFilter>("ALL");
  const [selectedService, setSelectedService] = useState("ALL");
  const [timeWindow, setTimeWindow] = useState<TimeWindow>("1h");
  const [expandedLogId, setExpandedLogId] = useState<string | undefined>(platformLogs[0]?.id);
  const [copiedKey, setCopiedKey] = useState<string | null>(null);

  const loadPlatformLogs = useCallback(async ({ silent = false } = {}) => {
    const controller = new AbortController();
    const timeoutId = window.setTimeout(() => controller.abort(), 8000);

    if (!silent) {
      setIsRefreshing(true);
    }

    try {
      const response = await fetch("/api/v1/platform/logs?limit=200", {
        signal: controller.signal
      });

      if (!response.ok) {
        throw new Error(`Log API returned ${response.status}`);
      }

      const data = (await response.json()) as PlatformLogsResponse;
      const liveLogs = normalizeApiLogs(data.items);

      setLogs(liveLogs);
      setExpandedLogId((current) =>
        liveLogs.some((log) => log.id === current) ? current : liveLogs[0]?.id
      );
      setDataSource("live");
      setLoadError(null);
      setLastUpdated(new Date());
    } catch (error) {
      setDataSource((current) => (current === "live" ? "live" : "sample"));
      setLoadError(error instanceof Error ? error.message : "Unable to load platform logs");
    } finally {
      window.clearTimeout(timeoutId);
      if (!silent) {
        setIsRefreshing(false);
      }
    }
  }, []);

  useEffect(() => {
    void loadPlatformLogs();

    if (!autoRefresh) {
      return undefined;
    }

    const interval = window.setInterval(() => void loadPlatformLogs({ silent: true }), 15000);
    return () => window.clearInterval(interval);
  }, [autoRefresh, loadPlatformLogs]);

  const services = useMemo(() => getServices(logs), [logs]);
  const stats = useMemo(() => getLogStats(logs), [logs]);
  const visibleLogs = useMemo(
    () =>
      filterLogs(logs, {
        level: selectedLevel,
        query,
        quickFilter: "all",
        service: selectedService,
        timeWindow
      }),
    [logs, query, selectedLevel, selectedService, timeWindow]
  );

  async function handleCopy(key: string, value: string) {
    try {
      await navigator.clipboard?.writeText(value);
      setCopiedKey(key);
      window.setTimeout(() => setCopiedKey(null), 1200);
    } catch {
      setCopiedKey(null);
    }
  }

  return (
    <main className="app-shell">
      <TopBar
        commit={logs[0]?.commit ?? "local"}
        dataSource={dataSource}
        isRefreshing={isRefreshing}
        lastUpdated={lastUpdated}
        loadError={loadError}
        stats={stats}
      />
      <CommandBar
        logs={logs}
        autoRefresh={autoRefresh}
        isRefreshing={isRefreshing}
        onAutoRefreshChange={setAutoRefresh}
        onLevelChange={setSelectedLevel}
        onQueryChange={setQuery}
        onRefresh={() => void loadPlatformLogs()}
        onServiceChange={setSelectedService}
        onTimeWindowChange={setTimeWindow}
        query={query}
        selectedLevel={selectedLevel}
        selectedService={selectedService}
        services={services}
        timeWindow={timeWindow}
      />
      <LogStream
        copiedKey={copiedKey}
        expandedLogId={expandedLogId}
        logs={visibleLogs}
        onCopy={handleCopy}
        onToggle={(logId) => setExpandedLogId((current) => (current === logId ? undefined : logId))}
      />
    </main>
  );
}
