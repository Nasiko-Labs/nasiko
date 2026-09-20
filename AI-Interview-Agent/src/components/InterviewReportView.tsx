import React from 'react';
import {
  Award,
  CheckCircle2,
  AlertTriangle,
  FileText,
  ArrowRight,
  TrendingUp,
  BrainCircuit,
  Repeat,
} from 'lucide-react';
import { InterviewReport } from '../types';

interface InterviewReportViewProps {
  report: InterviewReport;
  candidateName?: string;
  targetRole?: string;
  onGeneratePlan: () => Promise<void>;
  generatingPlan: boolean;
  hasExistingPlan: boolean;
  onViewPlan: () => void;
}

export const InterviewReportView: React.FC<InterviewReportViewProps> = ({
  report,
  candidateName,
  targetRole,
  onGeneratePlan,
  generatingPlan,
  hasExistingPlan,
  onViewPlan,
}) => {
  const getScoreColor = (score: number) => {
    if (score >= 8.0) return 'text-emerald-400 border-emerald-500/30 bg-emerald-950/20';
    if (score >= 6.0) return 'text-amber-400 border-amber-500/30 bg-amber-950/20';
    return 'text-rose-400 border-rose-500/30 bg-rose-950/20';
  };

  const getRatingLabel = (score: number) => {
    if (score >= 8.5) return 'Exceptional - Ready for Senior / Lead Placement';
    if (score >= 7.0) return 'Solid - Meets Core Expectations with Minor Coaching';
    if (score >= 5.5) return 'Developing - Good Foundation, Needs Focused Prep';
    return 'Needs Foundational Review Before Re-interviewing';
  };

  return (
    <div className="space-y-8 max-w-5xl mx-auto">
      {/* Report Header Card */}
      <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 sm:p-8 shadow-xl relative overflow-hidden">
        <div className="flex flex-col md:flex-row items-start md:items-center justify-between gap-6">
          <div className="space-y-2">
            <div className="inline-flex items-center space-x-2 px-3 py-1 rounded-full bg-indigo-950 text-indigo-300 border border-indigo-800 text-xs font-medium">
              <Award className="w-3.5 h-3.5" />
              <span>Multi-Agent Phase 4: Final Interview Report</span>
            </div>
            <h1 className="text-2xl sm:text-3xl font-bold text-white">
              Interview Performance Evaluation
            </h1>
            <p className="text-xs sm:text-sm text-slate-400">
              Candidate: <span className="text-slate-200 font-medium">{candidateName || 'Candidate'}</span> · Role:{' '}
              <span className="text-slate-200 font-medium">{targetRole || 'Software Engineer'}</span>
            </p>
          </div>

          {/* Overall Score Badge */}
          <div className="flex items-center space-x-4 bg-slate-950/70 border border-slate-800 p-4 rounded-2xl">
            <div className="text-right">
              <span className="text-[11px] font-semibold text-slate-400 uppercase tracking-wider block">
                Overall Rating
              </span>
              <span className="text-xs text-slate-500 max-w-[140px] block truncate">
                {getRatingLabel(report.overall_score)}
              </span>
            </div>
            <div
              className={`w-16 h-16 rounded-2xl border flex items-center justify-center font-mono text-2xl font-black ${getScoreColor(
                report.overall_score
              )}`}
            >
              {report.overall_score}
            </div>
          </div>
        </div>

        {/* 3 Core Dimensions */}
        <div className="grid grid-cols-1 sm:grid-cols-3 gap-4 mt-8 pt-6 border-t border-slate-800 text-xs">
          <div className="bg-slate-950/60 p-4 rounded-xl border border-slate-800 space-y-1">
            <span className="text-slate-400 font-medium">Technical Knowledge</span>
            <div className="flex items-baseline space-x-2">
              <span className="text-xl font-bold text-white font-mono">{report.technical_score}</span>
              <span className="text-slate-500">/ 10</span>
            </div>
            <div className="w-full bg-slate-800 h-1.5 rounded-full overflow-hidden mt-2">
              <div
                className="bg-indigo-500 h-full rounded-full"
                style={{ width: `${(report.technical_score / 10) * 100}%` }}
              />
            </div>
          </div>

          <div className="bg-slate-950/60 p-4 rounded-xl border border-slate-800 space-y-1">
            <span className="text-slate-400 font-medium">Communication & Structure</span>
            <div className="flex items-baseline space-x-2">
              <span className="text-xl font-bold text-white font-mono">{report.communication_score}</span>
              <span className="text-slate-500">/ 10</span>
            </div>
            <div className="w-full bg-slate-800 h-1.5 rounded-full overflow-hidden mt-2">
              <div
                className="bg-cyan-500 h-full rounded-full"
                style={{ width: `${(report.communication_score / 10) * 100}%` }}
              />
            </div>
          </div>

          <div className="bg-slate-950/60 p-4 rounded-xl border border-slate-800 space-y-1">
            <span className="text-slate-400 font-medium">Problem Solving & Architecture</span>
            <div className="flex items-baseline space-x-2">
              <span className="text-xl font-bold text-white font-mono">{report.problem_solving_score}</span>
              <span className="text-slate-500">/ 10</span>
            </div>
            <div className="w-full bg-slate-800 h-1.5 rounded-full overflow-hidden mt-2">
              <div
                className="bg-purple-500 h-full rounded-full"
                style={{ width: `${(report.problem_solving_score / 10) * 100}%` }}
              />
            </div>
          </div>
        </div>
      </div>

      {/* Category Performance Breakdown */}
      {report.category_scores && Object.keys(report.category_scores).length > 0 && (
        <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 shadow-md space-y-4">
          <div className="flex items-center space-x-2">
            <TrendingUp className="w-4 h-4 text-cyan-400" />
            <h2 className="text-sm font-semibold text-white">Category Mastery Breakdown</h2>
          </div>

          <div className="space-y-3">
            {Object.entries(report.category_scores).map(([category, score]) => (
              <div key={category} className="space-y-1">
                <div className="flex items-center justify-between text-xs">
                  <span className="text-slate-300 font-medium capitalize">
                    {category.replace(/_/g, ' ')}
                  </span>
                  <span className="font-mono text-slate-400 font-bold">{score} / 10</span>
                </div>
                <div className="w-full bg-slate-950 h-2 rounded-full overflow-hidden border border-slate-800">
                  <div
                    className={`h-full rounded-full transition-all duration-700 ${
                      score >= 7.5 ? 'bg-emerald-500' : score >= 5.5 ? 'bg-amber-500' : 'bg-rose-500'
                    }`}
                    style={{ width: `${Math.min(100, Math.max(5, (score / 10) * 100))}%` }}
                  />
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Cross-Question Patterns Detected Card */}
      {report.repeated_patterns_detected && report.repeated_patterns_detected.length > 0 && (
        <div className="bg-slate-900 border border-indigo-900/50 rounded-2xl p-6 shadow-md space-y-3">
          <div className="flex items-center space-x-2 text-indigo-400">
            <Repeat className="w-4 h-4" />
            <h2 className="text-sm font-semibold text-white">
              Agent 4 Multi-Question Pattern Analysis
            </h2>
          </div>
          <div className="space-y-2">
            {report.repeated_patterns_detected.map((pattern, i) => (
              <div
                key={i}
                className="bg-indigo-950/20 border border-indigo-800/40 rounded-xl p-3 text-xs text-indigo-200 leading-relaxed flex items-start space-x-2"
              >
                <BrainCircuit className="w-4 h-4 text-indigo-400 flex-shrink-0 mt-0.5" />
                <span>{pattern}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Strengths & Weaknesses 2-Column Grid */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-6 text-xs">
        <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 shadow-md space-y-3">
          <div className="flex items-center space-x-2 text-emerald-400 font-semibold text-sm">
            <CheckCircle2 className="w-4 h-4" />
            <span>Demonstrated Strengths</span>
          </div>
          <ul className="space-y-2">
            {(Array.isArray(report.strengths) ? report.strengths : []).map((s, idx) => (
              <li
                key={idx}
                className="bg-slate-950/70 border border-emerald-900/30 rounded-lg p-2.5 text-slate-200 leading-relaxed flex items-start space-x-2"
              >
                <span className="w-1.5 h-1.5 rounded-full bg-emerald-400 mt-1.5 flex-shrink-0" />
                <span>{s}</span>
              </li>
            ))}
          </ul>
        </div>

        <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 shadow-md space-y-3">
          <div className="flex items-center space-x-2 text-amber-400 font-semibold text-sm">
            <AlertTriangle className="w-4 h-4" />
            <span>Priority Areas to Improve</span>
          </div>
          <ul className="space-y-2">
            {(Array.isArray(report.weaknesses) ? report.weaknesses : []).map((w, idx) => (
              <li
                key={idx}
                className="bg-slate-950/70 border border-amber-900/30 rounded-lg p-2.5 text-slate-200 leading-relaxed flex items-start space-x-2"
              >
                <span className="w-1.5 h-1.5 rounded-full bg-amber-400 mt-1.5 flex-shrink-0" />
                <span>{w}</span>
              </li>
            ))}
          </ul>
        </div>
      </div>

      {/* Executive Narrative Feedback */}
      <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 sm:p-8 space-y-4 shadow-md">
        <div className="flex items-center space-x-2">
          <FileText className="w-4 h-4 text-indigo-400" />
          <h2 className="text-sm font-semibold text-white">Executive Interview Summary</h2>
        </div>
        <p className="text-xs sm:text-sm text-slate-300 leading-relaxed whitespace-pre-line">
          {report.interview_summary}
        </p>

        {report.detailed_feedback && (
          <div className="pt-4 border-t border-slate-800 space-y-2">
            <h3 className="text-xs font-semibold text-slate-400 uppercase tracking-wider">
              Detailed Technical Observations
            </h3>
            <p className="text-xs sm:text-sm text-slate-300 leading-relaxed whitespace-pre-line">
              {report.detailed_feedback}
            </p>
          </div>
        )}
      </div>

      {/* Action CTA for Agent 5 */}
      <div className="bg-gradient-to-r from-indigo-950/60 via-slate-900 to-indigo-950/60 border border-indigo-700/50 rounded-2xl p-6 sm:p-8 flex flex-col sm:flex-row items-center justify-between gap-6 shadow-2xl">
        <div className="space-y-1 text-center sm:text-left">
          <h3 className="text-lg font-bold text-white">Next Step: Turn Feedback into Action</h3>
          <p className="text-xs sm:text-sm text-indigo-200/80 max-w-xl">
            Launch Agent 5 (Personalized Preparation Plan Agent) to construct a day-by-day study roadmap, coding exercises, and targeted revision tasks.
          </p>
        </div>

        {hasExistingPlan ? (
          <button
            onClick={onViewPlan}
            className="px-6 py-3 rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white font-medium text-xs sm:text-sm flex items-center space-x-2 shadow-lg shadow-indigo-500/30 transition-all flex-shrink-0"
          >
            <span>View 7-Day Study Plan</span>
            <ArrowRight className="w-4 h-4" />
          </button>
        ) : (
          <button
            onClick={onGeneratePlan}
            disabled={generatingPlan}
            className="px-6 py-3 rounded-xl bg-gradient-to-r from-indigo-600 to-cyan-600 hover:from-indigo-500 hover:to-cyan-500 disabled:opacity-50 text-white font-medium text-xs sm:text-sm flex items-center space-x-2 shadow-lg shadow-indigo-500/30 transition-all flex-shrink-0"
          >
            {generatingPlan ? (
              <>
                <span className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                <span>Agent 5 Generating Personalized Plan...</span>
              </>
            ) : (
              <>
                <span>Generate Preparation Plan (Agent 5)</span>
                <ArrowRight className="w-4 h-4" />
              </>
            )}
          </button>
        )}
      </div>
    </div>
  );
};
