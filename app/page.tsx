'use client'

import { useMemo, useState } from 'react'
import { ArrowDown, Check, CircleAlert, Cpu, Gauge, GitBranch, Layers3, Play, Radio, RotateCcw, Server, ShieldCheck, Sparkles, Zap } from 'lucide-react'

type Result = {
  request_type: string
  complexity: number
  confidence: number
  classifier_backend: string
  fallback_used: boolean
  latency_us: number
  selected_tier?: string
}

const scenarios = [
  { label: 'Factual lookup', value: 'What is REST API?' },
  { label: 'Code generation', value: 'Write a Python FastAPI endpoint for user registration.' },
  { label: 'Code understanding', value: 'Explain why this recursive Python function causes stack overflow.' },
  { label: 'Technical design', value: 'Design a distributed payment system supporting 10 million users.' },
  { label: 'Analytical reasoning', value: 'Compare PostgreSQL and MongoDB for this application.' },
]

export default function Page() {
  const [query, setQuery] = useState(scenarios[3].value)
  const [simulateFailure, setSimulateFailure] = useState(false)
  const [result, setResult] = useState<Result | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const classify = async () => {
    setLoading(true)
    setError(null)
    try {
      const response = await fetch('/api/classify', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ query, simulate_failure: simulateFailure }),
      })
      const payload = await response.json().catch(() => ({}))
      if (!response.ok) throw new Error(payload.error ?? `Classifier adapter returned HTTP ${response.status}`)
      setResult(payload)
    } catch (nextError) {
      setResult(null)
      setError(nextError instanceof Error ? nextError.message : 'Classifier API unavailable')
    } finally {
      setLoading(false)
    }
  }

  const confidence = useMemo(() => result ? Math.round(result.confidence * 100) : null, [result])

  return (
    <main className="min-h-screen bg-[#081019] text-slate-100">
      <div className="mx-auto flex min-h-screen max-w-7xl flex-col px-5 py-6 sm:px-8 lg:px-10">
        <header className="flex items-center justify-between border-b border-white/10 pb-6">
          <div className="flex items-center gap-3">
            <div className="flex size-10 items-center justify-center rounded-xl bg-cyan-400/10 text-cyan-300 ring-1 ring-cyan-300/20"><Radio /></div>
            <div><p className="text-sm font-semibold tracking-[0.24em] text-cyan-300">NASIKO</p><p className="text-xs text-slate-500">Cost-aware routing lab</p></div>
          </div>
          <div className="flex items-center gap-2 rounded-full border border-emerald-300/20 bg-emerald-300/5 px-3 py-1.5 text-xs text-emerald-300"><span className="size-1.5 rounded-full bg-emerald-300" /> live router surface</div>
        </header>

        <section className="grid flex-1 gap-10 py-12 lg:grid-cols-[1.05fr_.95fr] lg:items-center">
          <div>
            <div className="mb-5 inline-flex items-center gap-2 rounded-full border border-white/10 bg-white/[.03] px-3 py-1.5 text-xs text-slate-400"><Sparkles className="size-3.5 text-cyan-300" /> P2 · Request classifier</div>
            <h1 className="max-w-2xl text-4xl font-semibold tracking-tight text-white sm:text-6xl">Route every request to the right model.</h1>
            <p className="mt-5 max-w-xl text-base leading-7 text-slate-400">A model-agnostic classification layer feeds Nasiko&apos;s existing tier selector without changing its sticky continuation behavior.</p>

            <div className="mt-9 rounded-2xl border border-white/10 bg-white/[.035] p-4 shadow-2xl shadow-cyan-950/20">
              <textarea aria-label="Request to classify" value={query} onChange={(event) => setQuery(event.target.value)} className="min-h-36 w-full resize-y bg-transparent p-2 text-sm leading-6 text-slate-200 outline-none placeholder:text-slate-600" placeholder="Describe the request to classify..." />
              <div className="mt-3 flex flex-col gap-3 border-t border-white/10 pt-3 sm:flex-row sm:items-center sm:justify-between">
                <div className="flex flex-wrap gap-2">{scenarios.slice(0, 3).map((scenario) => <button key={scenario.label} onClick={() => setQuery(scenario.value)} className="rounded-md border border-white/10 px-2.5 py-1.5 text-xs text-slate-400 transition hover:border-cyan-300/40 hover:text-cyan-200">{scenario.label}</button>)}</div>
                <button onClick={classify} disabled={loading || !query.trim()} className="inline-flex items-center justify-center gap-2 rounded-lg bg-cyan-300 px-4 py-2.5 text-sm font-semibold text-slate-950 transition hover:bg-cyan-200 disabled:cursor-not-allowed disabled:opacity-50"><Play className="size-4" /> {loading ? 'Classifying…' : 'Classify request'}</button>
              </div>
            </div>
            <label className="mt-4 flex cursor-pointer items-center gap-3 text-xs text-slate-400"><input type="checkbox" checked={simulateFailure} onChange={(event) => setSimulateFailure(event.target.checked)} className="size-4 accent-cyan-300" /> Simulate classifier failure and exercise the real fallback path</label>
            {error && <div className="mt-4 flex items-center gap-2 rounded-lg border border-amber-300/20 bg-amber-300/5 px-3 py-2 text-xs text-amber-200"><CircleAlert className="size-4" /> {error}. Start the Nasiko adapter to see live results.</div>}
          </div>

          <div className="space-y-4">
            <div className="grid gap-4 sm:grid-cols-2">
              <Metric icon={<GitBranch />} label="Request type" value={result?.request_type ?? 'No data yet'} />
              <Metric icon={<Gauge />} label="Complexity" value={result ? `${result.complexity} / 5` : 'No data yet'} />
              <Metric icon={<ShieldCheck />} label="Confidence" value={confidence === null ? 'No data yet' : `${confidence}%`} />
              <Metric icon={<Zap />} label="Latency" value={result ? `${(result.latency_us / 1000).toFixed(2)} ms` : 'No data yet'} />
            </div>
            <div className="rounded-2xl border border-white/10 bg-white/[.035] p-5">
              <div className="mb-5 flex items-center justify-between"><div><p className="text-sm font-medium text-white">Routing decision</p><p className="mt-1 text-xs text-slate-500">Classification feeds the existing Nasiko router</p></div><Server className="size-5 text-cyan-300" /></div>
              <FlowStep icon={<Radio />} label="Request classifier" value={result?.classifier_backend ?? 'Waiting for request'} active={Boolean(result)} />
              <ArrowDown className="my-1 ml-4 size-4 text-slate-600" />
              <FlowStep icon={<Layers3 />} label="Nasiko tier selection" value={result?.selected_tier ?? 'Existing routing flow'} active={Boolean(result)} />
              <ArrowDown className="my-1 ml-4 size-4 text-slate-600" />
              <FlowStep icon={<Cpu />} label="LLM" value={result ? (result.fallback_used ? 'Regex fallback' : 'Selected provider model') : 'Awaiting classification'} active={Boolean(result)} />
              {result && <div className={`mt-5 flex items-center gap-2 border-t border-white/10 pt-4 text-xs ${result.fallback_used ? 'text-amber-200' : 'text-emerald-300'}`}><Check className="size-4" /> {result.fallback_used ? 'Model error recovered by regex baseline' : 'No fallback used'}</div>}
            </div>
          </div>
        </section>

        <section className="border-t border-white/10 py-8"><div className="flex flex-col gap-5 lg:flex-row lg:items-end lg:justify-between"><div><p className="text-xs font-semibold uppercase tracking-[0.22em] text-slate-500">Live scenarios</p><h2 className="mt-2 text-xl font-medium text-white">Send real examples through the same classifier</h2></div><button onClick={() => { setResult(null); setError(null); setQuery('') }} className="inline-flex items-center gap-2 self-start text-xs text-slate-400 hover:text-white"><RotateCcw className="size-3.5" /> Clear result</button></div><div className="mt-5 grid gap-3 sm:grid-cols-2 lg:grid-cols-5">{scenarios.map((scenario) => <button key={scenario.label} onClick={() => setQuery(scenario.value)} className="rounded-xl border border-white/10 bg-white/[.025] p-4 text-left transition hover:-translate-y-0.5 hover:border-cyan-300/40"><p className="text-xs font-medium text-cyan-200">{scenario.label}</p><p className="mt-2 line-clamp-2 text-xs leading-5 text-slate-500">{scenario.value}</p></button>)}</div></section>
      </div>
    </main>
  )
}

function Metric({ icon, label, value }: { icon: React.ReactNode; label: string; value: string }) { return <div className="rounded-2xl border border-white/10 bg-white/[.035] p-4"><div className="flex items-center gap-2 text-slate-500">{icon}<span className="text-xs">{label}</span></div><p className="mt-4 truncate text-lg font-medium text-white">{value}</p></div> }
function FlowStep({ icon, label, value, active }: { icon: React.ReactNode; label: string; value: string; active: boolean }) { return <div className={`flex items-center gap-3 rounded-xl border p-3 ${active ? 'border-cyan-300/30 bg-cyan-300/5' : 'border-white/10 bg-black/10'}`}><div className="text-cyan-300">{icon}</div><div className="min-w-0 flex-1"><p className="text-xs text-slate-500">{label}</p><p className="truncate text-sm text-slate-200">{value}</p></div>{active && <span className="size-2 rounded-full bg-cyan-300" />}</div> }
