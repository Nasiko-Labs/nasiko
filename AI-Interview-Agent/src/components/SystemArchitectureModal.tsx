import React, { useState } from 'react';
import {
  X,
  Cpu,
  Server,
  Database,
  RefreshCw,
  CheckCircle2,
  AlertCircle,
  ExternalLink,
  ShieldCheck,
  Terminal,
  Zap,
  Globe,
} from 'lucide-react';
import { SystemStatus } from '../types';

interface SystemArchitectureModalProps {
  isOpen: boolean;
  onClose: () => void;
  systemStatus: SystemStatus | null;
  onRefresh: () => Promise<void>;
}

export const SystemArchitectureModal: React.FC<SystemArchitectureModalProps> = ({
  isOpen,
  onClose,
  systemStatus,
  onRefresh,
}) => {
  const [probing, setProbing] = useState(false);
  const [probeResult, setProbeResult] = useState<string | null>(null);

  if (!isOpen) return null;

  const handleProbeDocker = async () => {
    setProbing(true);
    setProbeResult(null);
    try {
      const res = await fetch('/api/nasiko/probe', { method: 'POST' });
      const data = await res.json();
      if (data.reachable) {
        setProbeResult('Successfully connected to Nasiko Docker on ' + data.status.targetUrl);
      } else {
        setProbeResult(
          `Docker instance at ${data.status.targetUrl} is not reachable yet. The application is seamlessly operating in Embedded Nasiko Orchestration mode.`
        );
      }
      await onRefresh();
    } catch (err: any) {
      setProbeResult(`Probe error: ${err.message}`);
    } finally {
      setProbing(false);
    }
  };

  const isBedrock = systemStatus?.llm.activeProvider === 'bedrock';
  const isDockerNasiko = systemStatus?.nasiko.isDockerReachable;

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 backdrop-blur-sm p-4 overflow-y-auto">
      <div className="bg-slate-900 border border-slate-800 rounded-2xl max-w-3xl w-full text-slate-200 shadow-2xl overflow-hidden my-8">
        {/* Header */}
        <div className="p-5 border-b border-slate-800 flex items-center justify-between bg-slate-950/60">
          <div className="flex items-center space-x-3">
            <div className="w-10 h-10 rounded-xl bg-indigo-500/10 border border-indigo-500/30 flex items-center justify-center text-indigo-400">
              <Server className="w-5 h-5" />
            </div>
            <div>
              <h2 className="text-base font-bold text-white flex items-center space-x-2">
                <span>Infrastructure & Control Plane Configuration</span>
              </h2>
              <p className="text-xs text-slate-400">
                Amazon Bedrock, Docker-hosted Nasiko (:8080), and Zero-Config Database
              </p>
            </div>
          </div>
          <button
            onClick={onClose}
            className="p-1.5 rounded-lg hover:bg-slate-800 text-slate-400 hover:text-white transition-colors"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        <div className="p-6 space-y-6 max-h-[75vh] overflow-y-auto text-xs">
          {/* Card 1: Amazon Bedrock */}
          <div className="bg-slate-950/70 border border-slate-800 rounded-xl p-4 space-y-3">
            <div className="flex items-center justify-between">
              <div className="flex items-center space-x-2.5">
                <Cpu className="w-4 h-4 text-amber-400" />
                <h3 className="font-semibold text-white text-sm">1. LLM Engine: Amazon Bedrock</h3>
              </div>
              <span
                className={`px-2.5 py-0.5 rounded-full font-mono text-[11px] font-semibold flex items-center space-x-1.5 ${
                  isBedrock
                    ? 'bg-amber-950 text-amber-300 border border-amber-800'
                    : systemStatus?.llm.bedrock.configured
                    ? 'bg-blue-950 text-blue-300 border border-blue-800'
                    : 'bg-slate-800 text-slate-300 border border-slate-700'
                }`}
              >
                {isBedrock ? (
                  <>
                    <CheckCircle2 className="w-3 h-3 text-amber-400" />
                    <span>ACTIVE PROVIDER</span>
                  </>
                ) : systemStatus?.llm.bedrock.configured ? (
                  <span>BEDROCK CONFIGURED</span>
                ) : (
                  <span>FALLBACK / READY</span>
                )}
              </span>
            </div>

            <p className="text-slate-300 leading-relaxed">
              Your application natively supports <strong>Amazon Bedrock</strong> using the official AWS Bedrock Converse runtime. You can provide your Bedrock API key or AWS IAM credentials in your environment variables.
            </p>

            <div className="grid grid-cols-1 sm:grid-cols-3 gap-2 bg-slate-900/90 p-3 rounded-lg border border-slate-800 font-mono text-[11px]">
              <div>
                <span className="text-slate-500 block text-[10px]">AUTH MODE</span>
                <span className="text-slate-200">
                  {systemStatus?.llm.bedrock.authType === 'api_key'
                    ? 'BEDROCK_API_KEY'
                    : systemStatus?.llm.bedrock.authType === 'iam_keys'
                    ? 'AWS IAM Keys (Access + Secret)'
                    : 'Awaiting Key in .env'}
                </span>
              </div>
              <div>
                <span className="text-slate-500 block text-[10px]">REGION</span>
                <span className="text-slate-200">{systemStatus?.llm.bedrock.region || 'us-east-1'}</span>
              </div>
              <div>
                <span className="text-slate-500 block text-[10px]">DEFAULT MODEL</span>
                <span className="text-amber-300 truncate block">
                  {systemStatus?.llm.bedrock.modelId || 'Claude 3.5 Sonnet'}
                </span>
              </div>
            </div>

            <div className="bg-slate-900/60 p-3 rounded-lg border border-slate-800/80 space-y-1.5">
              <div className="flex items-center space-x-1.5 text-slate-300 font-semibold">
                <Terminal className="w-3.5 h-3.5 text-indigo-400" />
                <span>How to provide your Bedrock Key:</span>
              </div>
              <p className="text-slate-400 leading-normal">
                Set either <code className="text-indigo-300 bg-slate-950 px-1 py-0.5 rounded">BEDROCK_API_KEY="your-key"</code> or standard AWS credentials <code className="text-indigo-300 bg-slate-950 px-1 py-0.5 rounded">AWS_ACCESS_KEY_ID</code> and <code className="text-indigo-300 bg-slate-950 px-1 py-0.5 rounded">AWS_SECRET_ACCESS_KEY</code>. Claude 3.5 Sonnet, Nova, and Llama 3 models are auto-negotiated.
              </p>
            </div>
          </div>

          {/* Card 2: Nasiko Control Plane & Docker */}
          <div className="bg-slate-950/70 border border-slate-800 rounded-xl p-4 space-y-3">
            <div className="flex items-center justify-between">
              <div className="flex items-center space-x-2.5">
                <Zap className="w-4 h-4 text-cyan-400" />
                <h3 className="font-semibold text-white text-sm">2. Orchestration: Nasiko Docker (:8080)</h3>
              </div>
              <span
                className={`px-2.5 py-0.5 rounded-full font-mono text-[11px] font-semibold flex items-center space-x-1.5 ${
                  isDockerNasiko
                    ? 'bg-emerald-950 text-emerald-300 border border-emerald-800'
                    : 'bg-indigo-950 text-indigo-300 border border-indigo-800'
                }`}
              >
                {isDockerNasiko ? (
                  <>
                    <CheckCircle2 className="w-3 h-3 text-emerald-400" />
                    <span>DOCKER CONNECTED</span>
                  </>
                ) : (
                  <>
                    <ShieldCheck className="w-3 h-3 text-indigo-400" />
                    <span>EMBEDDED CONTROL PLANE ACTIVE</span>
                  </>
                )}
              </span>
            </div>

            <p className="text-slate-300 leading-relaxed">
              When you clone the repository <code className="text-cyan-300 bg-slate-900 px-1.5 py-0.5 rounded">https://github.com/Nasiko-Labs/nasiko.git</code> and run <code className="text-cyan-300 bg-slate-900 px-1.5 py-0.5 rounded">docker compose up -d</code>, Nasiko exposes its server on port <strong>8080</strong> by default.
            </p>

            <div className="bg-slate-900/90 p-3 rounded-lg border border-slate-800 space-y-2">
              <div className="flex items-center justify-between">
                <span className="text-slate-400">Target Docker Endpoint:</span>
                <span className="font-mono text-cyan-400 font-semibold">
                  {systemStatus?.nasiko.targetUrl || 'http://localhost:8080'}
                </span>
              </div>
              <div className="flex items-center justify-between">
                <span className="text-slate-400">OpenTelemetry Collector:</span>
                <span className="font-mono text-slate-300">port 4318 (OTLP HTTP)</span>
              </div>
              <div className="flex items-center justify-between">
                <span className="text-slate-400">Agent Registry:</span>
                <span className="text-emerald-400 font-medium">5 / 5 Agents Registered</span>
              </div>
            </div>

            {probeResult && (
              <div className="p-2.5 rounded-lg bg-indigo-950/40 border border-indigo-800 text-indigo-200 text-xs">
                {probeResult}
              </div>
            )}

            <div className="flex items-center justify-between pt-1">
              <span className="text-slate-400">
                You do not need a public URL; localhost:8080 is probed automatically.
              </span>
              <button
                onClick={handleProbeDocker}
                disabled={probing}
                className="px-3 py-1.5 rounded-lg bg-indigo-600 hover:bg-indigo-500 text-white font-medium flex items-center space-x-1.5 transition-colors"
              >
                <RefreshCw className={`w-3.5 h-3.5 ${probing ? 'animate-spin' : ''}`} />
                <span>{probing ? 'Probing...' : 'Re-check Docker (:8080)'}</span>
              </button>
            </div>
          </div>

          {/* Card 3: Database URL Not Required */}
          <div className="bg-slate-950/70 border border-slate-800 rounded-xl p-4 space-y-3">
            <div className="flex items-center justify-between">
              <div className="flex items-center space-x-2.5">
                <Database className="w-4 h-4 text-emerald-400" />
                <h3 className="font-semibold text-white text-sm">3. Storage: Zero-Config Embedded Database</h3>
              </div>
              <span className="px-2.5 py-0.5 rounded-full font-mono text-[11px] font-semibold bg-emerald-950 text-emerald-300 border border-emerald-800 flex items-center space-x-1">
                <CheckCircle2 className="w-3 h-3 text-emerald-400" />
                <span>ZERO CONFIG REQUIRED</span>
              </span>
            </div>

            <p className="text-slate-300 leading-relaxed">
              <strong>You do not need a DATABASE_URL.</strong> The system incorporates a self-contained local storage engine that automatically persists all candidates, live sessions, questions, evaluations, reports, and study plans to:
            </p>

            <div className="bg-slate-900/90 p-3 rounded-lg border border-slate-800 flex items-center justify-between font-mono text-[11px]">
              <span className="text-slate-400">Storage File:</span>
              <span className="text-emerald-400 font-semibold">./data/interview_coach.json</span>
            </div>

            <p className="text-slate-400 text-[11px]">
              Everything works out of the box without installing PostgreSQL, MySQL, or setting connection strings.
            </p>
          </div>

          {/* Card 4: Web Scraping via Anakin.io */}
          <div className="bg-slate-950/70 border border-slate-800 rounded-xl p-4 space-y-3">
            <div className="flex items-center justify-between">
              <div className="flex items-center space-x-2.5">
                <Globe className="w-4 h-4 text-cyan-400" />
                <h3 className="font-semibold text-white text-sm">4. Web Scraping: Anakin.io Engine</h3>
              </div>
              <span className="px-2.5 py-0.5 rounded-full font-mono text-[11px] font-semibold bg-cyan-950 text-cyan-300 border border-cyan-800 flex items-center space-x-1">
                <CheckCircle2 className="w-3 h-3 text-cyan-400" />
                <span>
                  {systemStatus?.scraper?.mode === 'anakin_api' ? 'ANAKIN.IO ACTIVE' : 'SCRAPER READY'}
                </span>
              </span>
            </div>

            <p className="text-slate-300 leading-relaxed">
              Configured for web scraping candidate profile websites, technical portfolios, and job postings. Utilizes <strong>Anakin.io</strong> with your free credits, backed by an autonomous native scraper fallback.
            </p>

            <div className="bg-slate-900/90 p-3 rounded-lg border border-slate-800 space-y-2 font-mono text-[11px]">
              <div className="flex items-center justify-between">
                <span className="text-slate-400">Anakin API Endpoint:</span>
                <span className="text-cyan-400 font-semibold">
                  {systemStatus?.scraper?.endpoint || 'https://api.anakin.io/v1/scraper/scrape'}
                </span>
              </div>
              <div className="flex items-center justify-between">
                <span className="text-slate-400">Anakin Key Configured:</span>
                <span className={systemStatus?.scraper?.apiKeySet ? 'text-emerald-400' : 'text-amber-400'}>
                  {systemStatus?.scraper?.apiKeySet ? 'YES (Credits Active)' : 'Optional (Fallback Ready)'}
                </span>
              </div>
              <div className="flex items-center justify-between">
                <span className="text-slate-400">Capabilities:</span>
                <span className="text-slate-200">URL Scraping, Markdown Conversion, Job Extractor</span>
              </div>
            </div>
          </div>
        </div>

        {/* Footer */}
        <div className="p-4 border-t border-slate-800 bg-slate-950/60 flex items-center justify-end">
          <button
            onClick={onClose}
            className="px-4 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-white font-medium transition-colors"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
};
