import Link from "next/link";
import { type ReactNode } from "react";
import { CropMarks } from "@/components/crop-marks";

export function SiteFrame({
  children,
  folio,
}: {
  children: ReactNode;
  folio: string;
}) {
  return (
    <div className="relative min-h-screen bg-paper text-ink">
      <div className="grain" aria-hidden />
      <CropMarks />
      <header className="relative z-10 flex items-center justify-between border-b border-rule px-[min(6vw,72px)] py-5">
        <Link href="/" className="font-display text-2xl tracking-[-0.04em]">
          AgentKV
          <span className="ml-3 font-mono text-[10px] uppercase tracking-[0.32em] text-mute">
            Form A
          </span>
        </Link>
        <nav className="flex items-center gap-6 font-mono text-[11px] uppercase tracking-[0.22em]">
          <Link href="/metrics" className="hover:text-forest">
            Metrics
          </Link>
          <a href="https://github.com/perfect7613/AgentKV" className="hover:text-forest">
            GitHub
          </a>
        </nav>
      </header>
      {children}
      <footer className="relative z-10 flex flex-col gap-4 border-t border-rule px-[min(6vw,72px)] py-8 font-mono text-[11px] uppercase leading-relaxed tracking-[0.16em] text-mute md:flex-row md:justify-between">
        <p>Nasiko → AgentKV → vLLM · Jev optional</p>
        <p>{folio}</p>
        <p>Estimates are not invoices</p>
      </footer>
    </div>
  );
}
