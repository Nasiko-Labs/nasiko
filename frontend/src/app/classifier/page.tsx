'use client';

import { useState, useEffect, useCallback, useRef } from 'react';
import Link from 'next/link';
import Navigation from '@/components/Navigation';
import Footer from '@/components/Footer';
import {
  NormalizedClassifierResult,
  ApiResponse,
  HealthCheckResponse,
} from '@/lib/types';

interface ExamplePrompt {
  id: string;
  category: string;
  badge: string;
  query: string;
  context?: string;
  note: string;
}

const EXAMPLE_PROMPTS: ExamplePrompt[] = [
  {
    id: 'simple',
    category: 'Simple',
    badge: 'LOOKUP',
    query: 'What is the HTTP 304 Not Modified status code used for?',
    note: 'Factual lookup · Low complexity',
  },
  {
    id: 'mixed',
    category: 'Mixed-Intent',
    badge: 'CODE + DESIGN',
    query:
      'Refactor this PostgreSQL connection pool to handle failovers with exponential backoff and explain the concurrency trade-offs.',
    context: 'Database client: sqlx (Rust). Target pool max size: 20 connections.',
    note: 'Code generation + Technical design',
  },
  {
    id: 'noisy',
    category: 'Noisy / Typo',
    badge: 'ROBUSTNESS',
    query: 'plz wrte me a quck pythn scrip 2 prse dirty csv and fix brokn timestamps thx',
    note: 'Slang & typo tolerance test',
  },
  {
    id: 'unfamiliar',
    category: 'Unfamiliar / Edge',
    badge: 'HIGH REASONING',
    query:
      'Compute the Frobenius trace for a supersingular elliptic curve over GF(2^255 - 19) given the Weierstrass equation.',
    note: 'Niche domain · Extreme reasoning',
  },
];

export default function ClassifierPage() {
  const [query, setQuery] = useState('');
  const [context, setContext] = useState('');
  const [showContext, setShowContext] = useState(false);
  const [demoMode, setDemoMode] = useState(false);

  const [isLoading, setIsLoading] = useState(false);
  const [result, setResult] = useState<NormalizedClassifierResult | null>(null);
  const [errorMessage, setErrorMessage] = useState<{ title: string; details?: string } | null>(null);
  const [validationError, setValidationError] = useState<string | null>(null);

  const [healthStatus, setHealthStatus] = useState<HealthCheckResponse | null>(null);
  const [showRawJson, setShowRawJson] = useState(false);

  const resultRef = useRef<HTMLDivElement | null>(null);

  // Check health on mount
  useEffect(() => {
    fetch('/api/health')
      .then((res) => res.json())
      .then((data: HealthCheckResponse) => setHealthStatus(data))
      .catch(() => setHealthStatus(null));
  }, []);

  const handleClassify = useCallback(
    async (e?: React.FormEvent) => {
      if (e) e.preventDefault();

      if (!query.trim()) {
        setValidationError('Request prompt cannot be empty. Please enter a prompt or pick an example.');
        return;
      }

      setValidationError(null);
      setErrorMessage(null);
      setIsLoading(true);

      try {
        const response = await fetch('/api/classify', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            query: query.trim(),
            context: context.trim() ? context.trim() : undefined,
            demo: demoMode,
          }),
        });

        const json: ApiResponse<NormalizedClassifierResult> = await response.json();

        if (!response.ok || !json.success || !json.data) {
          setErrorMessage({
            title: json.error || `Classification failed (HTTP ${response.status})`,
            details: json.details || 'No additional diagnostics returned from server.',
          });
          setResult(null);
        } else {
          setResult(json.data);
          setErrorMessage(null);
          if (typeof window !== 'undefined' && window.innerWidth < 1024) {
            resultRef.current?.scrollIntoView();
          }
        }
      } catch (err: unknown) {
        setErrorMessage({
          title: 'Network or client connection failure',
          details: (err as Error)?.message || 'Unable to communicate with /api/classify',
        });
        setResult(null);
      } finally {
        setIsLoading(false);
      }
    },
    [query, context, demoMode]
  );

  const applyExample = (ex: ExamplePrompt) => {
    setQuery(ex.query);
    if (ex.context) {
      setContext(ex.context);
      setShowContext(true);
    } else {
      setContext('');
    }
    setValidationError(null);
    setErrorMessage(null);
  };

  const handleReset = () => {
    setQuery('');
    setContext('');
    setResult(null);
    setErrorMessage(null);
    setValidationError(null);
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
      e.preventDefault();
      handleClassify();
    }
  };

  return (
    <div className="fixed inset-0 h-screen w-screen overflow-hidden bg-transparent text-white flex flex-col font-mono select-none">
      {/* Shared Top Navigation */}
      <Navigation
        demoMode={demoMode}
        onToggleDemoMode={() => setDemoMode((prev) => !prev)}
        railwayConfigured={healthStatus?.railwayUrlConfigured}
      />

      {/* Viewfinder Corner Framing Accents */}
      <div className="fixed top-14 left-0 w-8 h-8 lg:w-12 lg:h-12 border-t border-l border-white/30 z-20 pointer-events-none" />
      <div className="fixed top-14 right-0 w-8 h-8 lg:w-12 lg:h-12 border-t border-r border-white/30 z-20 pointer-events-none" />
      <div className="fixed bottom-9 left-0 w-8 h-8 lg:w-12 lg:h-12 border-b border-l border-white/30 z-20 pointer-events-none" />
      <div className="fixed bottom-9 right-0 w-8 h-8 lg:w-12 lg:h-12 border-b border-r border-white/30 z-20 pointer-events-none" />

      {/* Central Scrollable Workspace with native scrolling */}
      <main className="fixed inset-0 top-14 bottom-9 overflow-y-auto px-4 lg:px-8 py-6 z-10 scroll-container">
        <div className="max-w-6xl mx-auto space-y-6">
          {/* Breadcrumb / Back Link */}
          <div className="flex items-center justify-between border-b border-white/10 pb-3">
            <div className="flex items-center gap-2 text-xs">
              <Link
                href="/"
                className="text-white/50 hover:text-white transition-colors flex items-center gap-1 group"
              >
                <span>←</span>
                <span className="underline-offset-4 group-hover:underline">Overview</span>
              </Link>
              <span className="text-white/20">/</span>
              <span className="text-white font-semibold">Classifier Console</span>
            </div>

            <div className="flex items-center gap-2 text-[10px] text-white/40">
              <span>TRACK: P2</span>
              <span>•</span>
              <span>INPUT: PROMPT + CONTEXT</span>
            </div>
          </div>

          {/* Active Demo Mode Alert Banner */}
          {demoMode && (
            <div className="p-3 border border-amber-500/50 bg-amber-950/20 text-amber-200 text-xs flex items-center justify-between">
              <div className="flex items-center gap-2">
                <span className="w-2 h-2 rounded-full bg-amber-400 animate-pulse" />
                <span className="font-bold tracking-wider">[DEMO DATA — NOT LIVE]</span>
                <span className="opacity-90">
                  Simulating deterministic outputs for UI testing. Disable to query your live Railway endpoint.
                </span>
              </div>
              <button
                type="button"
                onClick={() => setDemoMode(false)}
                className="text-[10px] underline hover:text-white uppercase ml-4 cursor-pointer"
              >
                Disable
              </button>
            </div>
          )}

          {/* Error Banner */}
          {errorMessage && (
            <div
              role="alert"
              aria-live="polite"
              className="p-4 border border-rose-500/50 bg-rose-950/25 text-rose-200 text-xs space-y-1.5 animate-in fade-in"
            >
              <div className="flex items-center gap-2 font-bold tracking-wider text-rose-400">
                <span className="w-2 h-2 rounded-full bg-rose-500" />
                <span>ROUTING ERROR: {errorMessage.title}</span>
              </div>
              {errorMessage.details && (
                <div className="text-[11px] text-neutral-300 font-mono pl-4 border-l border-rose-500/40 mt-1">
                  {errorMessage.details}
                </div>
              )}
              {!healthStatus?.railwayUrlConfigured && !demoMode && (
                <div className="pt-2 text-[10px] text-amber-300">
                  Tip: If your Railway classifier is still deploying, toggle{' '}
                  <button
                    type="button"
                    onClick={() => setDemoMode(true)}
                    className="underline font-bold text-white hover:text-amber-200 cursor-pointer"
                  >
                    DEMO MODE
                  </button>{' '}
                  to preview full results telemetry.
                </div>
              )}
            </div>
          )}

          {/* Main Grid: Form (Left) & Results (Right) */}
          <div className="grid grid-cols-1 lg:grid-cols-12 gap-6 items-start">
            {/* Left Column: Request Form & Examples (7 cols on lg) */}
            <div className="lg:col-span-7 space-y-4">
              <form onSubmit={handleClassify} className="space-y-4">
                {/* Request Prompt Container */}
                <div className="border border-white/20 bg-black/55 p-4 space-y-2 backdrop-blur-md">
                  <div className="flex items-center justify-between text-[10px] text-white/60">
                    <label htmlFor="prompt-input" className="font-semibold tracking-wider text-white">
                      REQUEST PROMPT <span className="text-rose-400">*</span>
                    </label>
                    <span className="text-white/40">[Ctrl + Enter to send]</span>
                  </div>

                  <textarea
                    id="prompt-input"
                    rows={5}
                    value={query}
                    onChange={(e) => {
                      setQuery(e.target.value);
                      if (validationError) setValidationError(null);
                    }}
                    onKeyDown={handleKeyDown}
                    placeholder="Enter query text to classify (e.g. 'Write a Python script that parses CSV', 'Calculate the probability...', 'What is the capital of France?')..."
                    className={`w-full bg-black/60 border ${
                      validationError ? 'border-rose-500' : 'border-white/20'
                    } p-3 text-xs sm:text-sm text-white placeholder-white/30 focus-visible:ring-1 focus-visible:ring-white focus-visible:outline-none resize-y transition-colors`}
                  />

                  {validationError && (
                    <div className="text-[11px] text-rose-400 flex items-center gap-1.5 pt-1">
                      <span>⚠</span>
                      <span>{validationError}</span>
                    </div>
                  )}

                  <div className="flex items-center justify-between text-[10px] text-white/40 pt-1">
                    <span>Target: Nasiko Adaptive Classifier</span>
                    <span>{query.length} chars</span>
                  </div>
                </div>

                {/* Optional Context Container */}
                <div className="border border-white/20 bg-black/55 p-3 space-y-2 backdrop-blur-md">
                  <div className="flex items-center justify-between">
                    <button
                      type="button"
                      onClick={() => setShowContext((prev) => !prev)}
                      className="text-[10px] text-white/70 hover:text-white flex items-center gap-1.5 font-medium tracking-wider cursor-pointer"
                    >
                      <span>{showContext ? '▼' : '▶'}</span>
                      <span>CONVERSATION CONTEXT (OPTIONAL)</span>
                    </button>
                    {context && (
                      <button
                        type="button"
                        onClick={() => setContext('')}
                        className="text-[9px] text-white/40 hover:text-white cursor-pointer"
                      >
                        CLEAR CONTEXT
                      </button>
                    )}
                  </div>

                  {showContext && (
                    <textarea
                      rows={3}
                      value={context}
                      onChange={(e) => setContext(e.target.value)}
                      placeholder="Optional conversation context, system instructions, or active tool schemas..."
                      className="w-full bg-black/60 border border-white/20 p-2.5 text-xs text-white placeholder-white/30 focus-visible:ring-1 focus-visible:ring-white focus-visible:outline-none resize-y transition-colors"
                    />
                  )}
                </div>

                {/* Form Buttons */}
                <div className="flex flex-wrap items-center gap-3 pt-1">
                  <button
                    type="submit"
                    disabled={isLoading}
                    className="relative px-6 py-2.5 bg-white text-black font-semibold text-xs tracking-wider border border-white hover:bg-neutral-200 transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed group active:scale-95"
                  >
                    <span className="hidden sm:block absolute -top-1 -left-1 w-2 h-2 border-t border-l border-white opacity-0 group-hover:opacity-100 transition-opacity" />
                    <span className="hidden sm:block absolute -bottom-1 -right-1 w-2 h-2 border-b border-r border-white opacity-0 group-hover:opacity-100 transition-opacity" />
                    {isLoading ? (
                      <span className="flex items-center gap-2">
                        <span className="w-2.5 h-2.5 border-2 border-black border-t-transparent rounded-full animate-spin" />
                        <span>CLASSIFYING...</span>
                      </span>
                    ) : (
                      <span>CLASSIFY REQUEST</span>
                    )}
                  </button>

                  <button
                    type="button"
                    onClick={handleReset}
                    disabled={isLoading || (!query && !context && !result)}
                    className="px-4 py-2.5 border border-white/20 text-white/60 hover:text-white hover:border-white/40 text-xs transition-colors cursor-pointer disabled:opacity-30 disabled:cursor-not-allowed"
                  >
                    RESET
                  </button>
                </div>
              </form>

              {/* Example Prompts Library */}
              <div className="border border-white/10 bg-black/45 p-4 space-y-3 backdrop-blur-sm">
                <div className="text-[10px] tracking-wider text-white/50 uppercase font-semibold">
                  Sample Evaluation Cases
                </div>
                <div className="grid grid-cols-1 sm:grid-cols-2 gap-2.5">
                  {EXAMPLE_PROMPTS.map((ex) => (
                    <button
                      key={ex.id}
                      type="button"
                      onClick={() => applyExample(ex)}
                      className="text-left p-2.5 border border-white/15 bg-white/5 hover:border-white/50 hover:bg-white/10 transition-all text-xs group cursor-pointer space-y-1"
                    >
                      <div className="flex items-center justify-between">
                        <span className="font-semibold text-white group-hover:text-white">
                          {ex.category}
                        </span>
                        <span className="text-[8px] px-1 py-0.5 border border-white/20 text-white/60">
                          {ex.badge}
                        </span>
                      </div>
                      <div className="text-[10px] text-neutral-400 line-clamp-1">
                        &quot;{ex.query}&quot;
                      </div>
                      <div className="text-[9px] text-white/40">{ex.note}</div>
                    </button>
                  ))}
                </div>
              </div>
            </div>

            {/* Right Column: Classification Results Panel (5 cols on lg) */}
            <div ref={resultRef} className="lg:col-span-5 space-y-4">
              {result ? (
                <div className="border border-white/30 bg-black/60 p-5 space-y-5 backdrop-blur-md relative animate-in fade-in">
                  {/* Result Header */}
                  <div className="flex items-center justify-between border-b border-white/20 pb-3">
                    <div className="flex items-center gap-2">
                      <span className="w-2 h-2 bg-emerald-400 rounded-full" />
                      <span className="text-xs font-bold tracking-widest text-white uppercase">
                        Classification Telemetry
                      </span>
                    </div>

                    <div className="flex items-center gap-2 text-[10px] text-white/60">
                      {result.latencyMs !== null && (
                        <span className="px-1.5 py-0.5 border border-white/20 bg-white/5">
                          {result.latencyMs} ms
                        </span>
                      )}
                      {result.classifier && (
                        <span className="px-1.5 py-0.5 border border-white/20 bg-white/5 truncate max-w-[120px]">
                          {result.classifier}
                        </span>
                      )}
                    </div>
                  </div>

                  {/* Fallback Warning Flag */}
                  {result.fallbackUsed && (
                    <div className="p-3 border border-amber-500 bg-amber-950/30 text-amber-300 text-xs space-y-1">
                      <div className="font-bold flex items-center gap-1.5 text-amber-400">
                        <span>⚠</span>
                        <span>FALLBACK PROTOCOL ENGAGED</span>
                      </div>
                      <p className="text-[11px] text-neutral-300">
                        Classifier could not achieve high-confidence tier voting. The query was routed through the default safe fallback rule.
                      </p>
                    </div>
                  )}

                  {/* Metric 1: Request Type */}
                  <div className="space-y-1">
                    <div className="text-[10px] text-white/50 tracking-wider uppercase">
                      Request Type
                    </div>
                    <div className="flex items-baseline gap-2">
                      <span className="text-lg font-bold text-white tracking-wide">
                        {result.requestTypeLabel}
                      </span>
                      <span className="text-[10px] text-white/40 px-1.5 py-0.5 border border-white/20 font-mono">
                        {result.requestType}
                      </span>
                    </div>
                    <p className="text-[11px] text-neutral-400 leading-relaxed">
                      {result.requestTypeDescription}
                    </p>
                  </div>

                  {/* Metric 2: Complexity Meter (1 - 5) */}
                  <div className="space-y-2 pt-1 border-t border-white/10">
                    <div className="flex items-center justify-between text-[10px]">
                      <span className="text-white/50 tracking-wider uppercase">Complexity Level</span>
                      <span className="font-bold text-white">
                        {result.complexity !== null ? `LEVEL ${result.complexity} / 5` : 'UNRATED'}
                      </span>
                    </div>

                    <div className="grid grid-cols-5 gap-1.5 h-3">
                      {[1, 2, 3, 4, 5].map((lvl) => {
                        const isFilled = result.complexity !== null && lvl <= result.complexity;
                        return (
                          <div
                            key={lvl}
                            className={`border border-white/30 transition-colors ${
                              isFilled ? 'bg-white shadow-[0_0_8px_rgba(255,255,255,0.4)]' : 'bg-transparent'
                            }`}
                          />
                        );
                      })}
                    </div>
                    <div className="flex justify-between text-[8px] text-white/40 tracking-wider">
                      <span>1 (TRIVIAL)</span>
                      <span>3 (MODERATE)</span>
                      <span>5 (EXHAUSTIVE REASONING)</span>
                    </div>
                  </div>

                  {/* Metric 3: Confidence Score */}
                  <div className="space-y-2 pt-1 border-t border-white/10">
                    <div className="flex items-center justify-between text-[10px]">
                      <span className="text-white/50 tracking-wider uppercase">Classifier Confidence</span>
                      <span className="font-bold text-white">
                        {result.confidencePct !== null
                          ? `${result.confidencePct}% (${result.confidence?.toFixed(3)})`
                          : 'N/A'}
                      </span>
                    </div>

                    <div className="w-full h-2 border border-white/30 bg-black/60 p-0.5">
                      <div
                        className="h-full bg-white transition-all duration-500"
                        style={{ width: `${result.confidencePct ?? 0}%` }}
                      />
                    </div>
                  </div>

                  {/* Metric 4: Model Tier Selection */}
                  {result.tier && (
                    <div className="space-y-1.5 pt-1 border-t border-white/10">
                      <div className="text-[10px] text-white/50 tracking-wider uppercase">
                        Selected AI Model Tier
                      </div>
                      <div className="p-3 border border-white/30 bg-white/5 flex items-center justify-between">
                        <div>
                          <div className="text-sm font-bold text-white">{result.tierLabel}</div>
                          <div className="text-[10px] text-neutral-400 mt-0.5">
                            {result.tier === 'tier_1' && 'Directs to frontier reasoning engines (e.g. Claude 3.5 Sonnet, GPT-4o).'}
                            {result.tier === 'tier_2' && 'Directs to balanced throughput models (e.g. GPT-4o-mini, Claude 3.5 Haiku).'}
                            {result.tier === 'tier_3' && 'Directs to ultra-low cost / nano models for basic lookups.'}
                          </div>
                        </div>
                        <span className="px-2 py-1 text-[10px] font-bold border border-white/40 bg-white/10 tracking-widest text-white">
                          {result.tier.toUpperCase()}
                        </span>
                      </div>
                    </div>
                  )}

                  {/* Baseline / Comparison Section (rendered ONLY if backend provided comparison) */}
                  {result.baseline && (
                    <div className="pt-2 border-t border-white/20 space-y-2">
                      <div className="text-[10px] text-white/50 tracking-wider uppercase flex items-center justify-between">
                        <span>Baseline Comparison</span>
                        <span className="text-[9px] text-white/30">Backend Benchmark</span>
                      </div>
                      <div className="p-3 border border-dashed border-white/30 bg-white/5 text-xs space-y-2">
                        <div className="grid grid-cols-2 gap-2 text-[11px]">
                          <div>
                            <span className="text-white/40 text-[9px] block">BASELINE TYPE</span>
                            <span className="text-white font-medium">
                              {result.baseline.requestTypeLabel}
                            </span>
                          </div>
                          <div>
                            <span className="text-white/40 text-[9px] block">BASELINE TIER</span>
                            <span className="text-white font-medium">
                              {result.baseline.tierLabel || result.baseline.tier || 'N/A'}
                            </span>
                          </div>
                          <div>
                            <span className="text-white/40 text-[9px] block">CONFIDENCE</span>
                            <span className="text-white font-medium">
                              {result.baseline.confidencePct !== null
                                ? `${result.baseline.confidencePct}%`
                                : 'N/A'}
                            </span>
                          </div>
                          <div>
                            <span className="text-white/40 text-[9px] block">LATENCY</span>
                            <span className="text-white font-medium">
                              {result.baseline.latencyMs !== null
                                ? `${result.baseline.latencyMs} ms`
                                : 'N/A'}
                            </span>
                          </div>
                        </div>
                      </div>
                    </div>
                  )}

                  {/* Raw JSON Accordion for Engineering / Inspection */}
                  <div className="pt-2 border-t border-white/10">
                    <button
                      type="button"
                      onClick={() => setShowRawJson((prev) => !prev)}
                      className="text-[10px] text-white/50 hover:text-white flex items-center gap-1.5 cursor-pointer"
                    >
                      <span>{showRawJson ? '▼ HIDE' : '▶ VIEW'} RAW BACKEND TELEMETRY</span>
                    </button>

                    {showRawJson && (
                      <pre className="mt-2 p-3 bg-black/80 border border-white/20 text-[10px] text-neutral-300 overflow-x-auto max-h-48">
                        {JSON.stringify(result.raw, null, 2)}
                      </pre>
                    )}
                  </div>
                </div>
              ) : (
                /* Idle State Placeholder */
                <div className="border border-dashed border-white/20 p-8 text-center space-y-3 bg-black/40">
                  <div className="w-8 h-8 mx-auto border border-white/30 flex items-center justify-center text-white/40">
                    ?
                  </div>
                  <div className="text-xs font-bold tracking-wider text-white">
                    AWAITING REQUEST SUBMISSION
                  </div>
                  <p className="text-[11px] text-neutral-400 max-w-xs mx-auto leading-relaxed">
                    Submit a query or click one of the sample evaluation cases to observe classification, complexity scoring, and tier routing.
                  </p>
                </div>
              )}
            </div>
          </div>

          {/* Section: How to Read This (Plain English Specification) */}
          <div className="border-t border-white/10 pt-6 pb-4">
            <h2 className="text-xs font-bold tracking-widest text-white/70 uppercase mb-3">
              How to Read This · Routing Instrumentation
            </h2>
            <div className="grid grid-cols-1 md:grid-cols-2 lg:grid-cols-4 gap-3 text-xs">
              <div className="p-3 border border-white/10 bg-black/45 backdrop-blur-sm space-y-1">
                <div className="font-bold text-white text-[11px]">1. Request Type</div>
                <p className="text-[10px] text-neutral-400 leading-relaxed">
                  Identifies the underlying nature of the task (e.g. code generation, technical design, factual lookup, writing). It keys the bandit learning loop.
                </p>
              </div>

              <div className="p-3 border border-white/10 bg-black/45 backdrop-blur-sm space-y-1">
                <div className="font-bold text-white text-[11px]">2. Complexity (1–5)</div>
                <p className="text-[10px] text-neutral-400 leading-relaxed">
                  Estimates computational depth. Level 1 represents fast atomic answers; Level 5 requires deep multi-step reasoning and execution planning.
                </p>
              </div>

              <div className="p-3 border border-white/10 bg-black/45 backdrop-blur-sm space-y-1">
                <div className="font-bold text-white text-[11px]">3. Confidence (0–100%)</div>
                <p className="text-[10px] text-neutral-400 leading-relaxed">
                  The model’s probability estimate that the classified category is correct. Lower scores trigger cautious routing or safety fallback.
                </p>
              </div>

              <div className="p-3 border border-white/10 bg-black/45 backdrop-blur-sm space-y-1">
                <div className="font-bold text-white text-[11px]">4. Fallback Protocol</div>
                <p className="text-[10px] text-neutral-400 leading-relaxed">
                  Engaged when the classifier encounters ambiguity, high noise, or timeouts. It automatically forwards to a reliable general tier.
                </p>
              </div>
            </div>
          </div>
        </div>
      </main>

      {/* Shared Footer Telemetry Strip */}
      <Footer statusText="SYSTEM.READY" />
    </div>
  );
}
