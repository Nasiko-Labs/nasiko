import React from 'react';
import { Activity, Bot, ChevronRight, FileText, CheckCircle2, Cpu, Server, Globe } from 'lucide-react';
import { AppView, SystemStatus } from '../types';

interface NavbarProps {
  currentView: AppView;
  onNavigate: (view: AppView) => void;
  onOpenTraces: () => void;
  onOpenSystemStatus: () => void;
  systemStatus?: SystemStatus | null;
  candidateName?: string;
  sessionActive?: boolean;
}

export const Navbar: React.FC<NavbarProps> = ({
  currentView,
  onNavigate,
  onOpenTraces,
  onOpenSystemStatus,
  systemStatus,
  candidateName,
}) => {
  const steps: Array<{ id: AppView; label: string; agent: string }> = [
    { id: 'resume', label: '1. Resume Profile', agent: 'Resume Agent' },
    { id: 'interview', label: '2. Live Interview', agent: 'Interview & Eval Agents' },
    { id: 'report', label: '3. Performance Report', agent: 'Report Agent' },
    { id: 'plan', label: '4. Preparation Plan', agent: 'Prep Plan Agent' },
  ];

  const isBedrock = systemStatus?.llm.activeProvider === 'bedrock';
  const isDockerNasiko = systemStatus?.nasiko.isDockerReachable;
  const isAnakinReady = Boolean(systemStatus?.scraper);

  return (
    <header className="sticky top-0 z-40 bg-slate-900/95 backdrop-blur border-b border-slate-800 text-white">
      <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8">
        <div className="flex items-center justify-between h-16">
          {/* Logo & Brand */}
          <div className="flex items-center space-x-3 cursor-pointer" onClick={() => onNavigate('home')}>
            <div className="w-10 h-10 rounded-xl bg-gradient-to-tr from-indigo-600 via-indigo-500 to-cyan-400 flex items-center justify-center shadow-lg shadow-indigo-500/20">
              <Bot className="w-6 h-6 text-white" />
            </div>
            <div>
              <div className="flex items-center space-x-2">
                <span className="font-bold text-lg tracking-tight bg-gradient-to-r from-white via-slate-100 to-slate-300 bg-clip-text text-transparent">
                  AI Interview Coach
                </span>
                <span className="text-[10px] uppercase font-semibold tracking-wider px-2 py-0.5 rounded-full bg-indigo-900/60 text-indigo-300 border border-indigo-700/50">
                  Nasiko Multi-Agent
                </span>
              </div>
              <p className="text-xs text-slate-400 hidden sm:block">
                Production 5-Agent Technical Interview & Mentorship System
              </p>
            </div>
          </div>

          {/* Stepper Pipeline */}
          <nav className="hidden lg:flex items-center space-x-1">
            {steps.map((step, idx) => {
              const isActive = currentView === step.id;
              const isPast =
                (step.id === 'resume' && (currentView === 'interview' || currentView === 'report' || currentView === 'plan')) ||
                (step.id === 'interview' && (currentView === 'report' || currentView === 'plan')) ||
                (step.id === 'report' && currentView === 'plan');

              return (
                <React.Fragment key={step.id}>
                  {idx > 0 && <ChevronRight className="w-3.5 h-3.5 text-slate-600 mx-1" />}
                  <button
                    onClick={() => onNavigate(step.id)}
                    className={`flex items-center space-x-2 px-3 py-1.5 rounded-lg text-xs font-medium transition-all ${
                      isActive
                        ? 'bg-indigo-600 text-white shadow-sm shadow-indigo-500/30'
                        : isPast
                        ? 'text-emerald-400 hover:bg-slate-800'
                        : 'text-slate-400 hover:text-slate-200 hover:bg-slate-800/60'
                    }`}
                  >
                    {isPast ? (
                      <CheckCircle2 className="w-3.5 h-3.5 text-emerald-400" />
                    ) : (
                      <span
                        className={`w-2 h-2 rounded-full ${
                          isActive ? 'bg-white animate-pulse' : 'bg-slate-500'
                        }`}
                      />
                    )}
                    <span>{step.label}</span>
                  </button>
                </React.Fragment>
              );
            })}
          </nav>

          {/* Action Right: Infrastructure Badges & Nasiko Traces */}
          <div className="flex items-center space-x-2 sm:space-x-3">
            {candidateName && (
              <div className="hidden xl:flex items-center space-x-1.5 text-xs text-slate-300 bg-slate-800/80 px-2.5 py-1 rounded-md border border-slate-700">
                <FileText className="w-3.5 h-3.5 text-indigo-400" />
                <span className="font-medium truncate max-w-[120px]">{candidateName}</span>
              </div>
            )}

            {/* System Status Pill (Bedrock, Nasiko, Anakin) */}
            <button
              onClick={onOpenSystemStatus}
              className="flex items-center space-x-2 px-2.5 py-1.5 rounded-lg bg-slate-800/90 hover:bg-slate-800 text-slate-200 text-xs font-medium border border-slate-700/80 transition-colors shadow-sm"
              title="View Amazon Bedrock, Docker Nasiko & Anakin.io Status"
            >
              <Cpu className="w-3.5 h-3.5 text-amber-400" />
              <span className="hidden md:inline font-mono text-[11px]">
                {isBedrock ? 'Bedrock' : systemStatus?.llm.activeProvider === 'gemini' ? 'Gemini' : 'LLM'}
              </span>
              <span className="text-slate-500 hidden md:inline">|</span>
              <Server className={`w-3.5 h-3.5 ${isDockerNasiko ? 'text-emerald-400' : 'text-cyan-400'}`} />
              <span className="hidden sm:inline font-mono text-[11px]">
                {isDockerNasiko ? 'Docker :8080' : 'Nasiko'}
              </span>
              {isAnakinReady && (
                <>
                  <span className="text-slate-500 hidden lg:inline">|</span>
                  <Globe className="w-3.5 h-3.5 text-cyan-400 hidden lg:inline" />
                  <span className="hidden lg:inline font-mono text-[11px] text-cyan-300">
                    Anakin.io
                  </span>
                </>
              )}
            </button>

            {/* Nasiko Control Plane Trace Drawer Trigger */}
            <button
              onClick={onOpenTraces}
              className="flex items-center space-x-2 px-3 py-1.5 rounded-lg bg-indigo-600/90 hover:bg-indigo-600 text-white text-xs font-medium transition-colors shadow-sm shadow-indigo-600/20"
              title="View Nasiko Agent Contracts & Traces"
            >
              <Activity className="w-3.5 h-3.5 text-cyan-300 animate-pulse" />
              <span className="hidden sm:inline">Traces & Contracts</span>
            </button>
          </div>
        </div>
      </div>
    </header>
  );
};

