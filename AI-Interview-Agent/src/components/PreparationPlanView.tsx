import React, { useState } from 'react';
import {
  Calendar,
  CheckCircle,
  Clock,
  Code2,
  HelpCircle,
  Download,
  Copy,
  Check,
  Sparkles,
  BookOpen,
  ArrowLeft,
  RotateCcw,
} from 'lucide-react';
import { PreparationPlan } from '../types';

interface PreparationPlanViewProps {
  plan: PreparationPlan;
  candidateName?: string;
  onBackToReport: () => void;
  onRestartNewInterview: () => void;
}

export const PreparationPlanView: React.FC<PreparationPlanViewProps> = ({
  plan,
  candidateName,
  onBackToReport,
  onRestartNewInterview,
}) => {
  const [completedTasks, setCompletedTasks] = useState<Record<string, boolean>>({});
  const [copiedIndex, setCopiedIndex] = useState<number | null>(null);

  const toggleTask = (taskId: string) => {
    setCompletedTasks((prev) => ({ ...prev, [taskId]: !prev[taskId] }));
  };

  const handleCopyCode = (code: string, index: number) => {
    navigator.clipboard.writeText(code);
    setCopiedIndex(index);
    setTimeout(() => setCopiedIndex(null), 2000);
  };

  const handleDownloadMarkdown = () => {
    let md = `# Personalized Technical Preparation Plan\n`;
    md += `Candidate: ${candidateName || 'Candidate'}\nGenerated: ${new Date().toLocaleDateString()}\n\n`;
    md += `## Learning Priorities\n`;
    (plan.learning_priorities || []).forEach((p) => (md += `- ${p}\n`));
    md += `\n## Daily Study Roadmap\n`;
    (plan.study_plan || []).forEach((d) => {
      md += `### Day ${d.day}: ${d.topic} (${d.estimated_minutes || 45} mins)\n`;
      md += `**Goals:**\n`;
      (d.goals || []).forEach((g) => (md += `- ${g}\n`));
      md += `**Key Concepts:** ${(d.concepts || []).join(', ')}\n`;
      md += `**Practice Tasks:**\n`;
      (d.practice_tasks || []).forEach((t) => (md += `- [ ] ${t}\n`));
      md += `\n`;
    });
    md += `## Practice Interview Questions\n`;
    (plan.practice_questions || []).forEach((q, i) => (md += `${i + 1}. ${q}\n`));

    const blob = new Blob([md], { type: 'text/markdown' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `preparation_plan_${(candidateName || 'candidate').toLowerCase().replace(/\s+/g, '_')}.md`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div className="space-y-8 max-w-5xl mx-auto">
      {/* Header */}
      <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 sm:p-8 shadow-xl flex flex-col md:flex-row items-start md:items-center justify-between gap-6">
        <div className="space-y-2">
          <div className="inline-flex items-center space-x-2 px-3 py-1 rounded-full bg-emerald-950 text-emerald-300 border border-emerald-800 text-xs font-medium">
            <Sparkles className="w-3.5 h-3.5" />
            <span>Multi-Agent Phase 5: Personalized Preparation Plan</span>
          </div>
          <h1 className="text-2xl sm:text-3xl font-bold text-white">
            Targeted Technical Study Roadmap
          </h1>
          <p className="text-xs sm:text-sm text-slate-400">
            Tailored for <span className="text-slate-200 font-medium">{candidateName || 'Candidate'}</span> based on
            evaluated gaps and strengths.
          </p>
        </div>

        <div className="flex items-center space-x-3">
          <button
            onClick={handleDownloadMarkdown}
            className="px-4 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-200 text-xs font-medium border border-slate-700 transition-colors flex items-center space-x-1.5"
          >
            <Download className="w-3.5 h-3.5 text-indigo-400" />
            <span>Export Markdown Plan</span>
          </button>

          <button
            onClick={onRestartNewInterview}
            className="px-4 py-2 rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white text-xs font-medium transition-colors flex items-center space-x-1.5 shadow-md shadow-indigo-500/20"
          >
            <RotateCcw className="w-3.5 h-3.5" />
            <span>New Interview</span>
          </button>
        </div>
      </div>

      {/* Ranked Learning Priorities */}
      {plan.learning_priorities && plan.learning_priorities.length > 0 && (
        <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 shadow-md space-y-4">
          <div className="flex items-center space-x-2">
            <BookOpen className="w-4 h-4 text-indigo-400" />
            <h2 className="text-sm font-semibold text-white">Ranked Learning Priorities</h2>
          </div>

          <div className="grid grid-cols-1 sm:grid-cols-3 gap-3">
            {plan.learning_priorities.map((p, idx) => (
              <div
                key={idx}
                className="bg-slate-950/70 border border-slate-800 rounded-xl p-3.5 space-y-1.5"
              >
                <span className="text-[10px] font-bold text-indigo-400 uppercase tracking-wider block">
                  Priority {idx + 1}
                </span>
                <p className="text-xs font-medium text-slate-200 leading-relaxed">{p}</p>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Day-by-Day Study Plan */}
      <div className="space-y-4">
        <div className="flex items-center space-x-2">
          <Calendar className="w-4 h-4 text-cyan-400" />
          <h2 className="text-base font-semibold text-white">
            Day-by-Day Actionable Study Roadmap ({plan.study_plan.length} Days)
          </h2>
        </div>

        <div className="space-y-4">
          {plan.study_plan.map((dayPlan) => (
            <div
              key={dayPlan.day}
              className="bg-slate-900 border border-slate-800 rounded-2xl p-6 shadow-md space-y-4"
            >
              <div className="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-2 border-b border-slate-800 pb-3">
                <div className="flex items-center space-x-3">
                  <span className="w-7 h-7 rounded-lg bg-indigo-600/30 border border-indigo-500/40 text-indigo-300 flex items-center justify-center font-bold text-xs">
                    D{dayPlan.day}
                  </span>
                  <div>
                    <h3 className="text-sm font-bold text-white">{dayPlan.topic}</h3>
                    <p className="text-xs text-slate-400">{dayPlan.goals?.join(' · ')}</p>
                  </div>
                </div>

                <span className="text-xs font-mono text-slate-400 bg-slate-950 px-2.5 py-1 rounded-md border border-slate-800 flex items-center space-x-1.5">
                  <Clock className="w-3.5 h-3.5 text-slate-500" />
                  <span>{dayPlan.estimated_minutes} mins</span>
                </span>
              </div>

              {/* Concepts Tags */}
              {dayPlan.concepts && dayPlan.concepts.length > 0 && (
                <div className="flex flex-wrap gap-1.5 items-center">
                  <span className="text-[11px] text-slate-500 mr-1">Concepts to Master:</span>
                  {dayPlan.concepts.map((c) => (
                    <span
                      key={c}
                      className="text-[10px] bg-slate-950 text-indigo-300 border border-indigo-900/50 px-2 py-0.5 rounded font-mono"
                    >
                      {c}
                    </span>
                  ))}
                </div>
              )}

              {/* Practice Tasks with Interactive Checkboxes */}
              {dayPlan.practice_tasks && dayPlan.practice_tasks.length > 0 && (
                <div className="space-y-2 pt-2">
                  <span className="text-[11px] font-semibold text-slate-400 uppercase tracking-wider block">
                    Hands-on Practice Tasks
                  </span>
                  <div className="space-y-1.5">
                    {dayPlan.practice_tasks.map((task, ti) => {
                      const taskId = `day-${dayPlan.day}-task-${ti}`;
                      const isDone = completedTasks[taskId];
                      return (
                        <div
                          key={ti}
                          onClick={() => toggleTask(taskId)}
                          className={`flex items-start space-x-3 p-2.5 rounded-lg border text-xs cursor-pointer transition-colors ${
                            isDone
                              ? 'bg-emerald-950/20 border-emerald-800/40 text-emerald-300'
                              : 'bg-slate-950/40 border-slate-800 hover:border-slate-700 text-slate-300'
                          }`}
                        >
                          <input
                            type="checkbox"
                            checked={!!isDone}
                            onChange={() => {}}
                            className="mt-0.5 rounded border-slate-700 text-indigo-600 focus:ring-0"
                          />
                          <span className={`leading-relaxed ${isDone ? 'line-through opacity-70' : ''}`}>
                            {task}
                          </span>
                        </div>
                      );
                    })}
                  </div>
                </div>
              )}
            </div>
          ))}
        </div>
      </div>

      {/* Coding Exercises & Starter Code */}
      {plan.coding_exercises && plan.coding_exercises.length > 0 && (
        <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 shadow-md space-y-4">
          <div className="flex items-center space-x-2">
            <Code2 className="w-4 h-4 text-indigo-400" />
            <h2 className="text-sm font-semibold text-white">Targeted Coding Exercises</h2>
          </div>

          <div className="space-y-4">
            {plan.coding_exercises.map((ex, idx) => {
              const isObj = typeof ex === 'object';
              const title = isObj ? ex.title : `Exercise ${idx + 1}`;
              const description = isObj ? ex.description : String(ex);
              const starterPrompt = isObj ? ex.starter_prompt : '';
              const difficulty = isObj ? ex.difficulty : 'medium';

              return (
                <div
                  key={idx}
                  className="bg-slate-950 border border-slate-800 rounded-xl p-4 space-y-3"
                >
                  <div className="flex items-center justify-between">
                    <h3 className="text-xs font-bold text-white">{title}</h3>
                    {difficulty && (
                      <span className="text-[10px] uppercase font-mono px-2 py-0.5 rounded bg-slate-900 text-slate-400 border border-slate-800">
                        {difficulty}
                      </span>
                    )}
                  </div>
                  <p className="text-xs text-slate-300 leading-relaxed">{description}</p>

                  {starterPrompt && (
                    <div className="relative">
                      <div className="flex items-center justify-between text-[10px] font-mono text-slate-500 bg-slate-900/90 px-3 py-1.5 rounded-t-lg border-x border-t border-slate-800">
                        <span>Starter Code</span>
                        <button
                          onClick={() => handleCopyCode(starterPrompt, idx)}
                          className="flex items-center space-x-1 text-slate-400 hover:text-white"
                        >
                          {copiedIndex === idx ? (
                            <>
                              <Check className="w-3 h-3 text-emerald-400" />
                              <span className="text-emerald-400">Copied!</span>
                            </>
                          ) : (
                            <>
                              <Copy className="w-3 h-3" />
                              <span>Copy code</span>
                            </>
                          )}
                        </button>
                      </div>
                      <pre className="p-3 bg-slate-900 rounded-b-lg border border-slate-800 text-xs font-mono text-cyan-300 overflow-x-auto">
                        {starterPrompt}
                      </pre>
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Targeted Practice Questions */}
      {plan.practice_questions && plan.practice_questions.length > 0 && (
        <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 shadow-md space-y-4">
          <div className="flex items-center space-x-2">
            <HelpCircle className="w-4 h-4 text-amber-400" />
            <h2 className="text-sm font-semibold text-white">Practice Interview Questions for Next Round</h2>
          </div>

          <div className="space-y-2">
            {plan.practice_questions.map((q, idx) => (
              <div
                key={idx}
                className="bg-slate-950/60 border border-slate-800 rounded-xl p-3.5 text-xs text-slate-200 leading-relaxed flex items-start space-x-3"
              >
                <span className="w-5 h-5 rounded-full bg-slate-800 text-slate-400 font-bold flex items-center justify-center flex-shrink-0 text-[10px]">
                  {idx + 1}
                </span>
                <span>{q}</span>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Recommended Next Interview */}
      {plan.recommended_next_interview && (
        <div className="bg-gradient-to-r from-indigo-950/30 to-slate-900 border border-indigo-900/60 rounded-2xl p-6 shadow-md flex flex-col sm:flex-row items-center justify-between gap-4">
          <div className="space-y-1 text-center sm:text-left">
            <h3 className="text-sm font-bold text-white">Recommended Follow-up Simulation</h3>
            <p className="text-xs text-slate-400">
              Role: <span className="text-slate-200 font-medium">{plan.recommended_next_interview.suggested_role || 'Senior Engineer'}</span> · Target Difficulty:{' '}
              <span className="text-indigo-300 font-medium capitalize">{plan.recommended_next_interview.target_difficulty || 'medium-hard'}</span>
            </p>
          </div>

          <div className="flex items-center space-x-3">
            <button
              onClick={onBackToReport}
              className="px-4 py-2 rounded-xl bg-slate-800 hover:bg-slate-700 text-slate-300 text-xs font-medium border border-slate-700 flex items-center space-x-1.5"
            >
              <ArrowLeft className="w-3.5 h-3.5" />
              <span>Back to Report</span>
            </button>
            <button
              onClick={onRestartNewInterview}
              className="px-4 py-2 rounded-xl bg-indigo-600 hover:bg-indigo-500 text-white text-xs font-medium flex items-center space-x-1.5 shadow-md shadow-indigo-500/20"
            >
              <span>Practice Again</span>
            </button>
          </div>
        </div>
      )}
    </div>
  );
};
