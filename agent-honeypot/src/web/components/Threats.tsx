import type { ThreatFinding } from "../../shared/types.js";
import { SEV_BG } from "../ui.js";

export function Threats({ threats }: { threats: ThreatFinding[] }) {
  return (
    <div className="rounded-lg border border-edge bg-panel">
      <div className="px-4 py-3 border-b border-edge text-xs uppercase tracking-wider text-slate-400">
        Threats Detected
      </div>
      <div className="p-3 space-y-2">
        {threats.length === 0 && (
          <div className="text-slate-600 text-sm py-4 text-center">
            No threats detected.
          </div>
        )}
        {threats.map((t) => (
          <div
            key={t.id}
            className={`rounded border p-3 ${SEV_BG[t.severity]}`}
          >
            <div className="flex items-center justify-between">
              <span className="font-semibold text-sm">
                {t.severity === "critical" ? "🚨" : "⚠"} {t.title}
              </span>
              <span className="text-[10px] font-bold uppercase">
                {t.severity}
              </span>
            </div>
            <ul className="mt-2 space-y-0.5 text-xs opacity-90">
              {t.evidence.map((ev, i) => (
                <li key={i}>• {ev}</li>
              ))}
            </ul>
          </div>
        ))}
      </div>
    </div>
  );
}
