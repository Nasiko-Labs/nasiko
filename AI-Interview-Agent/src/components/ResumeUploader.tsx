import React, { useState, useEffect } from 'react';
import { Upload, FileText, Sparkles, ArrowRight, Check, AlertCircle, FileCode, Globe, Zap } from 'lucide-react';
import { SampleResume } from '../types';

interface ResumeUploaderProps {
  onAnalyze: (resumeText: string, fileName?: string) => Promise<void>;
  analyzing: boolean;
}

export const ResumeUploader: React.FC<ResumeUploaderProps> = ({ onAnalyze, analyzing }) => {
  const [activeTab, setActiveTab] = useState<'upload' | 'paste' | 'anakin'>('upload');
  const [pastedText, setPastedText] = useState('');
  const [selectedFile, setSelectedFile] = useState<File | null>(null);
  const [samples, setSamples] = useState<SampleResume[]>([]);
  const [selectedSampleId, setSelectedSampleId] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [extractingFile, setExtractingFile] = useState(false);

  // Anakin.io Web Scraping State
  const [scrapeUrlInput, setScrapeUrlInput] = useState('');
  const [scraping, setScraping] = useState(false);
  const [scrapeSuccess, setScrapeSuccess] = useState<string | null>(null);
  const [scraperStatus, setScraperStatus] = useState<{
    configured: boolean;
    apiKeySet: boolean;
    endpoint: string;
    hasCredits: boolean;
    features: string[];
    mode: string;
  } | null>(null);

  useEffect(() => {
    fetch('/api/resume/samples')
      .then((res) => res.json())
      .then((data) => setSamples(data.samples || []))
      .catch((err) => console.error('Failed to load samples:', err));

    fetch('/api/scraper/status')
      .then((res) => res.json())
      .then((data) => setScraperStatus(data))
      .catch((err) => console.warn('Failed to load scraper status:', err));
  }, []);

  const handleFileChange = async (file: File) => {
    setError(null);
    setSelectedFile(file);
    setSelectedSampleId(null);
    setExtractingFile(true);

    try {
      const formData = new FormData();
      formData.append('resume', file);

      const res = await fetch('/api/resume/upload', {
        method: 'POST',
        body: formData,
      });

      if (!res.ok) {
        const errData = await res.json();
        throw new Error(errData.error || 'Failed to extract text from file');
      }

      const data = await res.json();
      setPastedText(data.resume_text);
    } catch (err: any) {
      setError(err.message);
    } finally {
      setExtractingFile(false);
    }
  };

  const handleSelectSample = (sample: SampleResume) => {
    setSelectedSampleId(sample.id);
    setSelectedFile(null);
    setPastedText(sample.resumeText);
    setError(null);
  };

  const handleScrapeWithAnakin = async (urlToScrape?: string) => {
    const targetUrl = (urlToScrape || scrapeUrlInput).trim();
    if (!targetUrl) {
      setError('Please enter a valid URL to scrape (e.g. GitHub profile, personal portfolio, or job posting)');
      return;
    }

    try {
      new URL(targetUrl);
    } catch {
      setError('Please include http:// or https:// in the URL');
      return;
    }

    setError(null);
    setScrapeSuccess(null);
    setScraping(true);

    try {
      const res = await fetch('/api/scraper/url', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ url: targetUrl, render_js: true, extract_markdown: true }),
      });

      if (!res.ok) {
        const errData = await res.json();
        throw new Error(errData.error || 'Web scraping failed');
      }

      const data = await res.json();
      const extractedContent = `[Source URL: ${data.url}]\n[Title: ${data.title}]\n[Engine: ${data.provider === 'anakin.io' ? 'Anakin.io URL Scraper' : 'Native Scraper (Anakin Ready)'}]\n\n${data.textContent}`;

      setPastedText(extractedContent);
      setSelectedFile(null);
      setSelectedSampleId(null);
      setScrapeSuccess(
        `Scraped successfully in ${data.latencyMs}ms via ${
          data.provider === 'anakin.io' ? 'Anakin.io API' : 'Native Scraper (Anakin Ready)'
        }: "${data.title}"`
      );
    } catch (err: any) {
      setError(`Scraper error: ${err.message}`);
    } finally {
      setScraping(false);
    }
  };

  const handleSubmit = async () => {
    if (!pastedText.trim()) {
      setError('Please provide a resume by uploading a file, pasting text, or scraping a profile.');
      return;
    }
    setError(null);
    const fileName = selectedFile?.name || (selectedSampleId ? `${selectedSampleId}_resume.txt` : 'scraped_profile.txt');
    await onAnalyze(pastedText, fileName);
  };

  return (
    <div className="space-y-8">
      {/* Hero Section */}
      <div className="text-center max-w-2xl mx-auto space-y-3">
        <div className="inline-flex items-center space-x-2 px-3 py-1 rounded-full bg-indigo-500/10 border border-indigo-500/20 text-indigo-400 text-xs font-medium">
          <Sparkles className="w-3.5 h-3.5" />
          <span>Multi-Agent Phase 1: Resume Reading Agent</span>
        </div>
        <h1 className="text-3xl font-extrabold tracking-tight text-white sm:text-4xl">
          Personalized Technical Interview Coach
        </h1>
        <p className="text-slate-400 text-sm sm:text-base">
          Upload your resume or scrape an online profile via Anakin.io to extract verified technical skills.
          Our multi-agent pipeline tailors each question and grades answers in real time.
        </p>
      </div>

      {/* Instant Demo Candidates Selector */}
      <div className="bg-slate-900/60 border border-slate-800 rounded-2xl p-6">
        <div className="flex items-center justify-between mb-4">
          <div className="flex items-center space-x-2">
            <FileCode className="w-4 h-4 text-indigo-400" />
            <h2 className="text-sm font-semibold text-white">Instant Demo Profiles (1-Click Load)</h2>
          </div>
          <span className="text-xs text-slate-400">Recommended for quick testing</span>
        </div>

        <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
          {samples.map((sample) => {
            const isSelected = selectedSampleId === sample.id;
            return (
              <button
                key={sample.id}
                onClick={() => handleSelectSample(sample)}
                className={`text-left p-4 rounded-xl border transition-all ${
                  isSelected
                    ? 'border-indigo-500 bg-indigo-950/30 shadow-md shadow-indigo-500/10'
                    : 'border-slate-800 hover:border-slate-700 bg-slate-900/40 hover:bg-slate-900/80'
                }`}
              >
                <div className="flex items-center justify-between mb-1.5">
                  <div className="font-semibold text-sm text-white">{sample.name}</div>
                  {isSelected && <Check className="w-4 h-4 text-indigo-400" />}
                </div>
                <div className="text-xs font-medium text-indigo-400 mb-2">{sample.role}</div>
                <p className="text-xs text-slate-400 line-clamp-2 mb-3 leading-relaxed">
                  {sample.summary}
                </p>
                <div className="flex flex-wrap gap-1">
                  {sample.skillsPreview.slice(0, 3).map((sk) => (
                    <span
                      key={sk}
                      className="text-[10px] bg-slate-800 text-slate-300 px-2 py-0.5 rounded"
                    >
                      {sk}
                    </span>
                  ))}
                </div>
              </button>
            );
          })}
        </div>
      </div>

      {/* Main Upload / Paste / Anakin Scrape Box */}
      <div className="bg-slate-900 border border-slate-800 rounded-2xl overflow-hidden shadow-xl">
        <div className="flex border-b border-slate-800 bg-slate-950/40">
          <button
            onClick={() => setActiveTab('upload')}
            className={`flex-1 py-3 text-xs font-medium flex items-center justify-center space-x-2 border-b-2 transition-colors ${
              activeTab === 'upload'
                ? 'border-indigo-500 text-white bg-slate-900/60'
                : 'border-transparent text-slate-400 hover:text-slate-200'
            }`}
          >
            <Upload className="w-3.5 h-3.5" />
            <span>Upload Document</span>
          </button>
          <button
            onClick={() => setActiveTab('paste')}
            className={`flex-1 py-3 text-xs font-medium flex items-center justify-center space-x-2 border-b-2 transition-colors ${
              activeTab === 'paste'
                ? 'border-indigo-500 text-white bg-slate-900/60'
                : 'border-transparent text-slate-400 hover:text-slate-200'
            }`}
          >
            <FileText className="w-3.5 h-3.5" />
            <span>Paste Resume Text</span>
          </button>
          <button
            onClick={() => setActiveTab('anakin')}
            className={`flex-1 py-3 text-xs font-medium flex items-center justify-center space-x-2 border-b-2 transition-colors ${
              activeTab === 'anakin'
                ? 'border-cyan-500 text-cyan-300 bg-slate-900/60'
                : 'border-transparent text-slate-400 hover:text-slate-200'
            }`}
          >
            <Globe className="w-3.5 h-3.5 text-cyan-400" />
            <span>Web Scraping (Anakin.io)</span>
          </button>
        </div>

        <div className="p-6">
          {activeTab === 'upload' && (
            <div>
              <label
                onDragOver={(e) => {
                  e.preventDefault();
                  setDragOver(true);
                }}
                onDragLeave={() => setDragOver(false)}
                onDrop={(e) => {
                  e.preventDefault();
                  setDragOver(false);
                  if (e.dataTransfer.files?.[0]) {
                    handleFileChange(e.dataTransfer.files[0]);
                  }
                }}
                className={`border-2 border-dashed rounded-xl p-8 flex flex-col items-center justify-center cursor-pointer transition-colors ${
                  dragOver
                    ? 'border-indigo-500 bg-indigo-950/20'
                    : 'border-slate-700 hover:border-slate-600 bg-slate-950/30'
                }`}
              >
                <input
                  type="file"
                  accept=".pdf,.docx,.txt"
                  className="hidden"
                  onChange={(e) => e.target.files?.[0] && handleFileChange(e.target.files[0])}
                />
                <div className="w-12 h-12 rounded-full bg-slate-800 flex items-center justify-center text-slate-300 mb-3">
                  <Upload className="w-6 h-6 text-indigo-400" />
                </div>
                <div className="text-sm font-medium text-white">
                  {selectedFile ? selectedFile.name : 'Click to upload or drag & drop'}
                </div>
                <p className="text-xs text-slate-400 mt-1">
                  Supported formats: PDF, Microsoft Word (.docx), or plain text (.txt)
                </p>
                {extractingFile && (
                  <div className="mt-3 text-xs text-indigo-400 flex items-center space-x-2">
                    <span className="w-2 h-2 rounded-full bg-indigo-400 animate-ping" />
                    <span>Extracting resume text...</span>
                  </div>
                )}
              </label>

              {pastedText && (
                <div className="mt-4">
                  <div className="flex items-center justify-between text-xs text-slate-400 mb-1">
                    <span>Extracted Resume Text Preview</span>
                    <span>{pastedText.length} characters</span>
                  </div>
                  <textarea
                    value={pastedText}
                    onChange={(e) => setPastedText(e.target.value)}
                    rows={6}
                    className="w-full bg-slate-950 border border-slate-800 rounded-lg p-3 text-xs text-slate-300 font-mono focus:outline-none focus:border-indigo-500"
                  />
                </div>
              )}
            </div>
          )}

          {activeTab === 'paste' && (
            <div>
              <div className="flex items-center justify-between text-xs text-slate-400 mb-2">
                <span>Paste the complete plain text of your resume</span>
                <span>{pastedText.length} characters</span>
              </div>
              <textarea
                value={pastedText}
                onChange={(e) => setPastedText(e.target.value)}
                placeholder="Paste your resume content here including Education, Work Experience, Technical Skills, and Projects..."
                rows={12}
                className="w-full bg-slate-950 border border-slate-800 rounded-xl p-4 text-xs text-slate-200 font-mono focus:outline-none focus:border-indigo-500 leading-relaxed"
              />
            </div>
          )}

          {activeTab === 'anakin' && (
            <div className="space-y-4">
              <div className="bg-slate-950/60 border border-cyan-900/40 rounded-xl p-4 flex flex-col sm:flex-row items-start sm:items-center justify-between gap-3">
                <div className="space-y-1">
                  <div className="flex items-center space-x-2">
                    <span className="w-2 h-2 rounded-full bg-cyan-400 animate-pulse" />
                    <span className="text-xs font-semibold text-white">
                      Anakin.io Automated URL Scraper Engine
                    </span>
                    <span className="text-[10px] px-2 py-0.5 rounded-full bg-cyan-950 text-cyan-300 border border-cyan-800 font-medium">
                      Free Credits Enabled
                    </span>
                  </div>
                  <p className="text-xs text-slate-400">
                    Scrape candidate portfolios, GitHub profile pages, or live job descriptions to extract target technical competencies.
                  </p>
                </div>
                <div className="text-right flex-shrink-0">
                  <span className="text-[11px] font-mono text-cyan-300/90 block">
                    {scraperStatus?.mode === 'anakin_api' ? 'Active: Anakin.io API' : 'Active: Ready (Fallback Guaranteed)'}
                  </span>
                </div>
              </div>

              <div className="space-y-2">
                <label className="text-xs font-medium text-slate-300 block">
                  Target Webpage URL to Scrape
                </label>
                <div className="flex flex-col sm:flex-row gap-2">
                  <input
                    type="url"
                    value={scrapeUrlInput}
                    onChange={(e) => setScrapeUrlInput(e.target.value)}
                    placeholder="https://news.ycombinator.com or https://example.com/job"
                    className="flex-1 bg-slate-950 border border-slate-800 rounded-xl px-4 py-2.5 text-xs text-slate-200 focus:outline-none focus:border-cyan-500 font-mono"
                  />
                  <button
                    onClick={() => handleScrapeWithAnakin()}
                    disabled={scraping || !scrapeUrlInput.trim()}
                    className="px-5 py-2.5 rounded-xl bg-cyan-600 hover:bg-cyan-500 disabled:opacity-50 disabled:cursor-not-allowed text-white text-xs font-semibold flex items-center justify-center space-x-2 transition-all shadow-md shadow-cyan-600/20 flex-shrink-0"
                  >
                    {scraping ? (
                      <>
                        <span className="w-3.5 h-3.5 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                        <span>Scraping URL...</span>
                      </>
                    ) : (
                      <>
                        <Zap className="w-3.5 h-3.5" />
                        <span>Scrape with Anakin.io</span>
                      </>
                    )}
                  </button>
                </div>
              </div>

              {/* Preset quick links */}
              <div className="flex flex-wrap items-center gap-2 pt-1 text-xs">
                <span className="text-slate-500 text-[11px]">Quick test URLs:</span>
                <button
                  type="button"
                  onClick={() => {
                    setScrapeUrlInput('https://news.ycombinator.com');
                    handleScrapeWithAnakin('https://news.ycombinator.com');
                  }}
                  className="px-2.5 py-1 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 border border-slate-700 text-[11px] transition-colors"
                >
                  Hacker News Live
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setScrapeUrlInput('https://example.com');
                    handleScrapeWithAnakin('https://example.com');
                  }}
                  className="px-2.5 py-1 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 border border-slate-700 text-[11px] transition-colors"
                >
                  Example Domain
                </button>
              </div>

              {scrapeSuccess && (
                <div className="p-3 rounded-lg bg-emerald-950/40 border border-emerald-800 text-emerald-300 text-xs flex items-center space-x-2">
                  <Check className="w-4 h-4 flex-shrink-0" />
                  <span>{scrapeSuccess}</span>
                </div>
              )}

              {pastedText && (
                <div className="mt-4">
                  <div className="flex items-center justify-between text-xs text-slate-400 mb-1">
                    <span>Scraped Content Preview (Ready for Agent 1 Analysis)</span>
                    <span>{pastedText.length} characters</span>
                  </div>
                  <textarea
                    value={pastedText}
                    onChange={(e) => setPastedText(e.target.value)}
                    rows={6}
                    className="w-full bg-slate-950 border border-slate-800 rounded-lg p-3 text-xs text-slate-300 font-mono focus:outline-none focus:border-cyan-500"
                  />
                </div>
              )}
            </div>
          )}

          {error && (
            <div className="mt-4 p-3 rounded-lg bg-rose-950/40 border border-rose-800 text-rose-300 text-xs flex items-center space-x-2">
              <AlertCircle className="w-4 h-4 flex-shrink-0" />
              <span>{error}</span>
            </div>
          )}

          <div className="mt-6 flex justify-end">
            <button
              onClick={handleSubmit}
              disabled={analyzing || extractingFile || scraping || !pastedText.trim()}
              className="flex items-center space-x-2 px-6 py-3 rounded-xl bg-indigo-600 hover:bg-indigo-500 disabled:opacity-50 disabled:cursor-not-allowed text-white font-medium text-sm shadow-lg shadow-indigo-500/25 transition-all"
            >
              {analyzing ? (
                <>
                  <span className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                  <span>Agent 1 (Resume Reading Agent) Processing...</span>
                </>
              ) : (
                <>
                  <span>Extract & Analyze Profile with Agent 1</span>
                  <ArrowRight className="w-4 h-4" />
                </>
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};
