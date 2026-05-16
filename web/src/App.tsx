import { useMemo, useState } from "react";
import { CommandBar } from "./components/CommandBar";
import { LogStream } from "./components/LogStream";
import { TopBar } from "./components/TopBar";
import { platformLogs } from "./data/platformLogs";
import type { LevelFilter, TimeWindow } from "./types";
import { filterLogs, getLogStats, getServices } from "./utils/logs";

export function App() {
  const [query, setQuery] = useState("");
  const [selectedLevel, setSelectedLevel] = useState<LevelFilter>("ALL");
  const [selectedService, setSelectedService] = useState("ALL");
  const [timeWindow, setTimeWindow] = useState<TimeWindow>("1h");
  const [expandedLogId, setExpandedLogId] = useState<string | undefined>(platformLogs[0]?.id);
  const [copiedKey, setCopiedKey] = useState<string | null>(null);

  const services = useMemo(() => getServices(platformLogs), []);
  const stats = useMemo(() => getLogStats(platformLogs), []);
  const visibleLogs = useMemo(
    () =>
      filterLogs(platformLogs, {
        level: selectedLevel,
        query,
        quickFilter: "all",
        service: selectedService,
        timeWindow
      }),
    [query, selectedLevel, selectedService, timeWindow]
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
      <TopBar commit={platformLogs[0]?.commit ?? "local"} stats={stats} />
      <CommandBar
        logs={platformLogs}
        onLevelChange={setSelectedLevel}
        onQueryChange={setQuery}
        onRefresh={() => window.location.reload()}
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
