'use client';

import Link from 'next/link';
import { usePathname } from 'next/navigation';

interface NavigationProps {
  demoMode?: boolean;
  onToggleDemoMode?: () => void;
  railwayConfigured?: boolean;
}

export default function Navigation({
  demoMode,
  onToggleDemoMode,
  railwayConfigured,
}: NavigationProps) {
  const pathname = usePathname();
  const isClassifier = pathname === '/classifier';

  return (
    <header className="fixed top-0 left-0 right-0 z-30 h-14 border-b border-white/20 bg-black/85 backdrop-blur-md px-4 lg:px-8 flex items-center justify-between font-mono select-none">
      <div className="flex items-center gap-4 lg:gap-6">
        {/* Brand */}
        <Link
          href="/"
          className="flex items-center gap-2 group transition-opacity hover:opacity-90"
        >
          <span className="w-2 h-2 bg-white rounded-none rotate-45 inline-block group-hover:bg-neutral-300 transition-colors" />
          <span className="font-bold text-sm lg:text-base tracking-widest text-white italic transform -skew-x-12">
            NASIKO
          </span>
          <span className="text-white/40 text-xs">/</span>
          <span className="text-[11px] lg:text-xs text-white/80 font-semibold tracking-wider">
            ROUTER LAB
          </span>
        </Link>

        <div className="h-3 w-px bg-white/30 hidden sm:block" />

        {/* Navigation Tabs */}
        <nav className="flex items-center gap-1 sm:gap-2 text-[10px] tracking-wider uppercase">
          <Link
            href="/"
            className={`px-2.5 py-1 transition-colors border ${
              !isClassifier
                ? 'border-white text-white bg-white/10'
                : 'border-transparent text-white/60 hover:text-white hover:border-white/20'
            }`}
          >
            Overview
          </Link>
          <Link
            href="/classifier"
            className={`px-2.5 py-1 transition-colors border ${
              isClassifier
                ? 'border-white text-white bg-white/10'
                : 'border-transparent text-white/60 hover:text-white hover:border-white/20'
            }`}
          >
            Classifier Console
          </Link>
        </nav>
      </div>

      {/* Right Controls & Indicators */}
      <div className="flex items-center gap-3 lg:gap-4 text-[10px]">
        {/* Demo Mode Toggle (Classifier page) */}
        {onToggleDemoMode && (
          <button
            type="button"
            onClick={onToggleDemoMode}
            className={`px-2.5 py-1 border transition-colors flex items-center gap-1.5 cursor-pointer ${
              demoMode
                ? 'border-amber-400 bg-amber-950/40 text-amber-300'
                : 'border-white/20 bg-white/5 text-white/60 hover:border-white/40 hover:text-white'
            }`}
            title="Toggle simulated demo data for previewing without live Railway endpoint"
          >
            <span
              className={`w-1.5 h-1.5 rounded-full ${demoMode ? 'bg-amber-400 animate-pulse' : 'bg-white/40'}`}
            />
            <span>{demoMode ? 'DEMO: ON' : 'DEMO: OFF'}</span>
          </button>
        )}

        {/* Railway Status Indicator */}
        <div className="hidden sm:flex items-center gap-1.5 text-white/70">
          {railwayConfigured ? (
            <span className="flex items-center gap-1.5 text-emerald-400">
              <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-ping" />
              <span>RAILWAY: LIVE</span>
            </span>
          ) : (
            <span
              className="flex items-center gap-1.5 text-neutral-400"
              title="RAILWAY_API_URL is unconfigured. Set in .env.local or Vercel."
            >
              <span className="w-1.5 h-1.5 rounded-full bg-neutral-500" />
              <span>RAILWAY: UNSET</span>
            </span>
          )}
        </div>

        {/* Track Badge */}
        <span className="hidden md:inline-block px-2 py-0.5 border border-white/20 text-[9px] uppercase tracking-widest text-white/50 bg-white/5">
          P2 · ROUTER
        </span>
      </div>
    </header>
  );
}
