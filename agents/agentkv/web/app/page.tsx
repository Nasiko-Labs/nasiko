import Link from "next/link";
import { BeamCard, BentoCell, BentoGrid } from "@/components/ui/bento";
import { Marquee } from "@/components/ui/marquee";
import { Spotlight } from "@/components/ui/spotlight";
import { NumberTicker } from "@/components/ui/ticker";
import { SiteFrame } from "@/components/site-frame";
import { PUBLISHED } from "@/lib/report";

const NASIKO = process.env.NEXT_PUBLIC_NASIKO_URL;

export default function HomePage() {
  return (
    <SiteFrame folio="Sheet 01 · Landing">
      <section className="relative px-[min(6vw,72px)] pb-8 pt-16 md:pt-24">
        <p className="animate-rise font-mono text-[11px] uppercase tracking-[0.32em] text-mute">
          Recorded evidence · not a promised speedup
        </p>
        <h1 className="animate-rise mt-6 max-w-[14ch] font-display text-[clamp(3.4rem,11vw,9.2rem)] leading-[0.86] tracking-[-0.055em] [animation-delay:80ms]">
          The prefix
          <span className="italic text-forest"> already </span>
          paid.
        </h1>
        <p className="animate-rise mt-8 max-w-xl text-lg leading-relaxed text-mute [animation-delay:140ms] md:text-xl">
          AgentKV compiles stable prompts and bounded eviction preferences through Nasiko.
          This public desk shows the Qwen3.8-27B cohort that actually ran. Live agents,
          traces, and GPUs stay on Modal.
        </p>
        <div className="mt-10 flex flex-wrap items-center gap-4">
          <Link
            href="/metrics"
            className="bg-ink px-6 py-3 font-mono text-[11px] uppercase tracking-[0.24em] text-phosphor"
          >
            Open the metrics desk
          </Link>
          {NASIKO ? (
            <a
              href={NASIKO}
              className="border border-ink px-6 py-3 font-mono text-[11px] uppercase tracking-[0.24em]"
            >
              Nasiko live
            </a>
          ) : (
            <span className="border border-rule px-6 py-3 font-mono text-[11px] uppercase tracking-[0.24em] text-mute">
              Nasiko environment stopped
            </span>
          )}
        </div>
      </section>

      <Marquee
        items={[
          "Nasiko control plane",
          "Jev two-Noul bound",
          "vLLM prefix cache",
          "Qwen3.8-27B BF16",
          "A100 80 GB",
          "Methods A–E",
          "No simulated wins",
        ]}
      />

      <section className="px-[min(6vw,72px)] py-16">
        <div className="mb-8 flex items-end justify-between gap-6">
          <h2 className="font-display text-4xl tracking-[-0.04em] md:text-5xl">
            First cohort, as measured.
          </h2>
          <p className="hidden max-w-sm text-sm leading-relaxed text-mute md:block">
            Natural-memory, concurrency 4, 20 authored repairs × 5 methods × 3 repeats.
            Stock cache (A) had the lowest p95 and estimated unit cost.
          </p>
        </div>
        <div className="grid gap-px bg-rule sm:grid-cols-2 lg:grid-cols-4">
          <Stat label="Workflows passed" value={PUBLISHED.passed} suffix="/300" />
          <Stat label="A p95 latency" value={PUBLISHED.aP95} decimals={2} suffix="s" />
          <Stat label="A est. $/1k" value={PUBLISHED.aCost} decimals={2} prefix="$" />
          <Stat label="B–E cached tokens" value={PUBLISHED.reuseCache} suffix="%" />
        </div>
      </section>

      <section className="px-[min(6vw,72px)] pb-20">
        <BentoGrid>
          <BentoCell className="md:col-span-7 md:row-span-2">
            <Spotlight className="h-full">
              <p className="font-mono text-[10px] uppercase tracking-[0.28em] text-mute">01 / Control plane</p>
              <h3 className="mt-4 font-display text-4xl tracking-[-0.04em] md:text-5xl">
                Nasiko runs the team.
              </h3>
              <p className="mt-5 max-w-lg text-base leading-relaxed text-mute">
                Nasiko authenticates Coder, Tester, and Reviewer, pins Qwen in its model
                router, and keeps native traces. AgentKV does not replace that UI. When a
                demo is live, Nasiko is the operator surface. This Vercel app is the
                public companion for sanitized, recorded metrics.
              </p>
              <dl className="mt-8 grid gap-4 font-mono text-xs uppercase tracking-[0.16em] sm:grid-cols-3">
                <div>
                  <dt className="text-mute">Agents</dt>
                  <dd className="mt-1 text-sm tracking-normal">Coder · Tester · Reviewer</dd>
                </div>
                <div>
                  <dt className="text-mute">Router</dt>
                  <dd className="mt-1 text-sm tracking-normal">{PUBLISHED.model}</dd>
                </div>
                <div>
                  <dt className="text-mute">Host</dt>
                  <dd className="mt-1 text-sm tracking-normal">Modal, not Vercel</dd>
                </div>
              </dl>
            </Spotlight>
          </BentoCell>
          <BentoCell className="md:col-span-5">
            <p className="font-mono text-[10px] uppercase tracking-[0.28em] text-mute">02 / Compiler</p>
            <h3 className="mt-3 font-display text-3xl tracking-[-0.03em]">Stable prefixes.</h3>
            <p className="mt-4 text-sm leading-relaxed text-mute">
              Methods B–E put shared repository context in the system turn and the task
              in the user turn so decode work is comparable. Cache hits are engine-owned.
            </p>
          </BentoCell>
          <BentoCell className="bg-forest text-cream md:col-span-5">
            <p className="font-mono text-[10px] uppercase tracking-[0.28em] text-phosphor/80">03 / Honest result</p>
            <h3 className="mt-3 font-display text-3xl tracking-[-0.03em] text-phosphor">
              Reuse is not the product outcome.
            </h3>
            <p className="mt-4 text-sm leading-relaxed text-cream/75">
              B–E reached {PUBLISHED.reuseCache}% cached prompt tokens. A still won this
              cohort on p95 and estimated cost. The desk never invents a win from a hit rate.
            </p>
          </BentoCell>
        </BentoGrid>
      </section>

      <section className="px-[min(6vw,72px)] pb-24">
        <BeamCard>
          <div className="grid gap-10 p-8 md:grid-cols-[1.2fr_1fr] md:p-12">
            <div>
              <p className="font-mono text-[10px] uppercase tracking-[0.28em] text-mute">04 / Methods</p>
              <h3 className="mt-4 font-display text-4xl tracking-[-0.04em]">A through E, same engine.</h3>
              <ol className="mt-8 space-y-3 font-serif text-[15px] leading-relaxed text-mute">
                <li><span className="font-mono text-ink">A</span> — stock prefix cache, task-first prompts</li>
                <li><span className="font-mono text-ink">B</span> — stock cache, stable repository prefixes</li>
                <li><span className="font-mono text-ink">C</span> — B plus deterministic workflow retention</li>
                <li><span className="font-mono text-ink">D</span> — C with a disjoint-calibration repair frequency</li>
                <li><span className="font-mono text-ink">E</span> — C with two timely Jev Nouls; idle-GPU exact-prefix warm</li>
              </ol>
            </div>
            <div className="flex flex-col justify-between border-t border-rule pt-8 md:border-l md:border-t-0 md:pl-10 md:pt-0">
              <p className="text-sm leading-relaxed text-mute">
                Inspect workflows, engine acknowledgements, and the cost–latency scatter.
                Prompts, generated code, credentials, and cache hashes stay off this site.
              </p>
              <Link
                href="/metrics"
                className="mt-8 self-start bg-phosphor px-5 py-3 font-mono text-[11px] uppercase tracking-[0.22em] text-ink"
              >
                Enter the desk →
              </Link>
            </div>
          </div>
        </BeamCard>
      </section>
    </SiteFrame>
  );
}

function Stat({
  label,
  value,
  decimals = 0,
  prefix = "",
  suffix = "",
}: {
  label: string;
  value: number;
  decimals?: number;
  prefix?: string;
  suffix?: string;
}) {
  return (
    <div className="bg-cream px-6 py-8">
      <p className="font-mono text-[10px] uppercase tracking-[0.24em] text-mute">{label}</p>
      <p className="mt-4 font-display text-5xl tracking-[-0.04em]">
        <NumberTicker value={value} decimals={decimals} prefix={prefix} suffix={suffix} />
      </p>
    </div>
  );
}
