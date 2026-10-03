'use client';

interface FooterProps {
  statusText?: string;
}

export default function Footer({ statusText = 'SYSTEM.READY' }: FooterProps) {
  return (
    <footer className="fixed bottom-0 left-0 right-0 z-30 h-9 border-t border-white/20 bg-black/90 backdrop-blur-md px-4 lg:px-8 flex items-center justify-between text-[9px] text-white/50 font-mono select-none">
      <div className="flex items-center gap-3 sm:gap-4">
        <span className="font-semibold text-white/70">NASIKO LLM ROUTER</span>
        <span className="hidden sm:inline text-white/20">|</span>
        <span className="hidden sm:inline">P2 · REQUEST CLASSIFIER</span>
        <span className="hidden md:inline text-white/20">|</span>
        <span className="hidden md:inline text-white/40">V0.2.0</span>
      </div>

      <div className="flex items-center gap-3">
        <span className="hidden sm:inline">VERCEL + RAILWAY</span>
        <span className="hidden sm:inline text-white/20">•</span>
        <span className="text-emerald-400 flex items-center gap-1.5">
          <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 animate-pulse" />
          <span>{statusText}</span>
        </span>
      </div>
    </footer>
  );
}
