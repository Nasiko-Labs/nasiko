import React, { useEffect, useState } from 'react';
import { X, CheckCircle2, AlertCircle, Clock, Cpu, RefreshCw, Layers } from 'lucide-react';
import { NasikoStats, NasikoTrace } from '../types';

interface NasikoTraceDrawerProps {
  isOpen: boolean;
  onClose: () => void;
}

export const NasikoTraceDrawer: React.FC<NasikoTraceDrawerProps> = ({ isOpen, onClose }) => {
  const [stats, setStats] = useState<NasikoStats | null>(null);
  const [loading, setLoading] = useState(false);
  const [selectedTrace, setSelectedTrace] = useState<NasikoTrace | null>(null);

  const fetchStats = async () => {
    setLoading(true);
    try {
      const res = await fetch('/api/observability/stats');
      if (res.ok) {
        const data = await res.json();
        // Normalize backend spans (startedAt/inputSummary) to drawer shape
        // so traces render even if the API returns the raw contract.
        const normalized = {
          ...data,
          recentTraces: (data.recentTraces || []).map((t: any) => ({
            ...t,
            timestamp: t.timestamp || t.completedAt || t.startedAt || new Date().toISOString(),
            input: t.input ?? t.inputSummary ?? {},
            output: t.output ?? t.outputSummary ?? {},
          })),
        };
        setStats(normalized);
        if (normalized.recentTraces?.length > 0 && !selectedTrace) {
          setSelectedTrace(normalized.recentTraces[0]);
        }
      }
    } catch (err) {
      console.error('Failed to load Nasiko stats:', err);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (isOpen) {
      fetchStats();
      const interval = setInterval(fetchStats, 4000);
      return () => clearInterval(interval);
    }
  }, [isOpen]);

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-50 flex justify-end bg-black/60 backdrop-blur-sm transition-opacity">
      <div className="w-full max-w-3xl bg-slate-900 border-l border-slate-800 text-slate-200 h-full flex flex-col shadow-2xl">
        {/* Header */}
        <div className="p-4 border-b border-slate-800 flex items-center justify-between bg-slate-900/80">
          <div className="flex items-center space-x-3">
            <div className="w-8 h-8 rounded-lg bg-indigo-600/20 border border-indigo-500/30 flex items-center justify-center text-indigo-400">
              <Layers className="w-4 h-4" />
            </div>
            <div>
              <h2 className="text-sm font-semibold text-white flex items-center space-x-2">
                <span>Nasiko Agent Control Plane</span>
                <span className="text-[10px] bg-emerald-950 text-emerald-300 border border-emerald-800 px-1.5 py-0.5 rounded font-mono">
                  ACTIVE
                </span>
              </h2>
              <p className="text-xs text-slate-400">
                Multi-agent registry, runtime contracts, and telemetry
              </p>
            </div>
          </div>

          <div className="flex items-center space-x-2">
            <button
              onClick={fetchStats}
              disabled={loading}
              className="p-1.5 rounded-md hover:bg-slate-800 text-slate-400 hover:text-white transition-colors"
              title="Refresh telemetry"
            >
              <RefreshCw className={`w-4 h-4 ${loading ? 'animate-spin text-indigo-400' : ''}`} />
            </button>
            <button
              onClick={onClose}
              className="p-1.5 rounded-md hover:bg-slate-800 text-slate-400 hover:text-white transition-colors"
            >
              <X className="w-5 h-5" />
            </button>
          </div>
        </div>

        {/* Overview Cards */}
        {stats && (
          <div className="grid grid-cols-4 gap-2 p-4 border-b border-slate-800 bg-slate-950/50 text-xs">
            <div className="bg-slate-900/70 p-2.5 rounded-lg border border-slate-800">
              <span className="text-slate-400 block text-[11px]">Registered Agents</span>
              <span className="text-base font-semibold text-white">
                {stats.summary.activeAgentsCount} / 5
              </span>
            </div>
            <div className="bg-slate-900/70 p-2.5 rounded-lg border border-slate-800">
              <span className="text-slate-400 block text-[11px]">Total Agent Calls</span>
              <span className="text-base font-semibold text-indigo-400">
                {stats.summary.totalCalls}
              </span>
            </div>
            <div className="bg-slate-900/70 p-2.5 rounded-lg border border-slate-800">
              <span className="text-slate-400 block text-[11px]">Avg Duration</span>
              <span className="text-base font-semibold text-cyan-400">
                {stats.summary.avgLatencyMs} ms
              </span>
            </div>
            <div className="bg-slate-900/70 p-2.5 rounded-lg border border-slate-800">
              <span className="text-slate-400 block text-[11px]">Estimated Tokens</span>
              <span className="text-base font-semibold text-amber-400">
                {stats.summary.totalTokens}
              </span>
            </div>
          </div>
        )}

        {/* Content Tabs: Registered Agents + Live Trace Stream */}
        <div className="flex-1 overflow-y-auto p-4 space-y-4 text-xs">
          {/* Registered Agents Grid */}
          <div>
            <h3 className="text-xs font-semibold uppercase tracking-wider text-slate-400 mb-2">
              Agent Registry & Contracts
            </h3>
            <div className="space-y-2">
              {stats?.agents.map((agent, i) => (
                <div
                  key={agent.name}
                  className="bg-slate-900 border border-slate-800 rounded-lg p-3 flex items-center justify-between"
                >
                  <div className="space-y-1">
                    <div className="flex items-center space-x-2">
                      <span className="w-5 h-5 rounded-full bg-slate-800 text-[10px] font-bold text-slate-300 flex items-center justify-center">
                        {i + 1}
                      </span>
                      <span className="font-semibold text-white text-xs">{agent.displayName}</span>
                      <code className="text-[10px] text-slate-400 bg-slate-950 px-1.5 py-0.5 rounded">
                        {agent.name}
                      </code>
                    </div>
                    <p className="text-[11px] text-slate-400 pl-7">{agent.responsibility}</p>
                  </div>
                  <div className="flex items-center space-x-4 text-right">
                    <div>
                      <span className="text-[10px] text-slate-500 block">Calls / Avg</span>
                      <span className="font-mono text-slate-300">
                        {agent.totalExecutions} calls · {agent.avgDurationMs}ms
                      </span>
                    </div>
                    <CheckCircle2 className="w-4 h-4 text-emerald-400" />
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* Execution Traces */}
          <div>
            <h3 className="text-xs font-semibold uppercase tracking-wider text-slate-400 mb-2">
              Recent Nasiko Execution Traces ({stats?.recentTraces.length || 0})
            </h3>

            {(!stats?.recentTraces || stats.recentTraces.length === 0) && (
              <div className="text-center py-8 text-slate-500 border border-dashed border-slate-800 rounded-lg">
                No agent execution traces recorded yet. Upload a resume or answer an interview question to see live contracts.
              </div>
            )}

            <div className="space-y-2">
              {stats?.recentTraces.map((trace) => {
                const isSelected = selectedTrace?.id === trace.id;
                return (
                  <div
                    key={trace.id}
                    onClick={() => setSelectedTrace(trace)}
                    className={`border rounded-lg p-3 cursor-pointer transition-all ${
                      isSelected
                        ? 'border-indigo-500 bg-indigo-950/20'
                        : 'border-slate-800 bg-slate-900/60 hover:border-slate-700'
                    }`}
                  >
                    <div className="flex items-center justify-between">
                      <div className="flex items-center space-x-2">
                        {trace.status === 'success' ? (
                          <CheckCircle2 className="w-3.5 h-3.5 text-emerald-400" />
                        ) : (
                          <AlertCircle className="w-3.5 h-3.5 text-rose-400" />
                        )}
                        <span className="font-semibold text-slate-200">{trace.agentName}</span>
                        <span className="text-[10px] text-slate-500 font-mono">
                          {new Date(trace.timestamp).toLocaleTimeString()}
                        </span>
                      </div>
                      <div className="flex items-center space-x-3 text-[11px] font-mono text-slate-400">
                        <span className="flex items-center space-x-1">
                          <Clock className="w-3 h-3 text-slate-500" />
                          <span>{trace.durationMs}ms</span>
                        </span>
                        {trace.tokens && (
                          <span className="flex items-center space-x-1">
                            <Cpu className="w-3 h-3 text-slate-500" />
                            <span>{trace.tokens.totalTokens} tok</span>
                          </span>
                        )}
                      </div>
                    </div>
                  </div>
                );
              })}
            </div>
          </div>

          {/* Trace Contract Inspector */}
          {selectedTrace && (
            <div className="border border-slate-800 rounded-lg p-3 bg-slate-950/80 mt-4 space-y-3">
              <div className="flex items-center justify-between border-b border-slate-800 pb-2">
                <span className="font-semibold text-white">
                  Trace Contract Inspector: <code className="text-indigo-400">{selectedTrace.agentName}</code>
                </span>
                <span className="text-[10px] font-mono text-slate-500">ID: {selectedTrace.id}</span>
              </div>

              <div>
                <span className="text-[10px] uppercase font-bold text-slate-400 block mb-1">
                  Validated Input Contract
                </span>
                <pre className="bg-slate-900 p-2.5 rounded text-[11px] font-mono overflow-x-auto max-h-48 text-emerald-300 border border-slate-800">
                  {JSON.stringify(selectedTrace.input, null, 2)}
                </pre>
              </div>

              <div>
                <span className="text-[10px] uppercase font-bold text-slate-400 block mb-1">
                  Validated Output Contract
                </span>
                <pre className="bg-slate-900 p-2.5 rounded text-[11px] font-mono overflow-x-auto max-h-60 text-cyan-300 border border-slate-800">
                  {JSON.stringify(selectedTrace.output, null, 2)}
                </pre>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
