import type { SecurityEvent } from "../../shared/types.js";
import { fmtTime, outcomeColor, outcomeIcon, outcomeLabel } from "../ui.js";

export function Timeline({
  events,
  onSelect,
  selected,
}: {
  events: SecurityEvent[];
  onSelect: (e: SecurityEvent) => void;
  selected: SecurityEvent | null;
}) {
  return (
    <div className="rounded-lg border border-edge bg-panel">
      <div className="px-4 py-3 border-b border-edge text-xs uppercase tracking-wider text-slate-400">
        Live Agent Activity
      </div>
      <div className="max-h-[420px] overflow-y-auto divide-y divide-edge/50">
        {events.length === 0 && (
          <div className="px-4 py-10 text-center text-slate-600 text-sm">
            No activity yet. Run an attack to begin.
          </div>
        )}
        {events.map((e) => {
          const isBlocked = e.outcome === "blocked" || e.outcome === "rejected";
          return (
            <button
              key={e.id}
              onClick={() => onSelect(e)}
              className={`row-in w-full flex items-center gap-3 px-4 py-2.5 text-left text-sm hover:bg-panel2 transition ${
                selected?.id === e.id ? "bg-panel2" : ""
              }`}
            >
              <span className={`text-lg ${outcomeColor(e.outcome)}`}>
                {outcomeIcon(e.outcome)}
              </span>
              <span className="font-medium text-slate-200 w-40 truncate">
                {e.tool}
              </span>
              <span
                className={`text-[10px] font-bold px-1.5 py-0.5 rounded ${outcomeColor(
                  e.outcome,
                )} border border-current/30`}
              >
                {outcomeLabel(e.outcome)}
              </span>
              <span className="text-slate-500 text-xs flex-1 truncate">
                {String(e.telemetry.intent ?? "")}
              </span>
              {isBlocked && (
                <span className="text-[10px] text-accent">🛡 Nasiko</span>
              )}
              <span className="text-slate-600 text-xs">
                {fmtTime(e.timestamp)}
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
