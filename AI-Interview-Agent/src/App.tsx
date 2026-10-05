import React, { useState, useEffect } from 'react';
import {
  AppView,
  CandidateProfile,
  InterviewSession,
  InterviewQuestion,
  AnswerEvaluation,
  InterviewReport,
  PreparationPlan,
  SystemStatus,
} from './types';
import { Navbar } from './components/Navbar';
import { NasikoTraceDrawer } from './components/NasikoTraceDrawer';
import { SystemArchitectureModal } from './components/SystemArchitectureModal';
import { ResumeUploader } from './components/ResumeUploader';
import { CandidateProfileView } from './components/CandidateProfileView';
import { InterviewScreen } from './components/InterviewScreen';
import { InterviewReportView } from './components/InterviewReportView';
import { PreparationPlanView } from './components/PreparationPlanView';
import {
  Bot,
  Sparkles,
  ArrowRight,
  CheckCircle2,
  FileCheck2,
  Calendar,
  Layers,
  Play,
  Award,
  BookOpen,
  AlertCircle,
  X,
} from 'lucide-react';

export default function App() {
  const [currentView, setCurrentView] = useState<AppView>('home');
  const [isTraceOpen, setIsTraceOpen] = useState(false);
  const [isSystemStatusOpen, setIsSystemStatusOpen] = useState(false);
  const [systemStatus, setSystemStatus] = useState<SystemStatus | null>(null);

  // Core Multi-Agent State Pipeline
  const [candidateId, setCandidateId] = useState<string | null>(null);
  const [candidateProfile, setCandidateProfile] = useState<CandidateProfile | null>(null);
  const [analyzingResume, setAnalyzingResume] = useState(false);

  const [session, setSession] = useState<InterviewSession | null>(null);
  const [currentQuestion, setCurrentQuestion] = useState<InterviewQuestion | null>(null);
  const [questions, setQuestions] = useState<InterviewQuestion[]>([]);
  const [answers, setAnswers] = useState<Array<{ question_id: string; answer: string }>>([]);
  const [evaluations, setEvaluations] = useState<AnswerEvaluation[]>([]);
  const [startingInterview, setStartingInterview] = useState(false);
  const [submittingAnswer, setSubmittingAnswer] = useState(false);

  const [report, setReport] = useState<InterviewReport | null>(null);
  const [generatingReport, setGeneratingReport] = useState(false);

  const [plan, setPlan] = useState<PreparationPlan | null>(null);
  const [generatingPlan, setGeneratingPlan] = useState(false);

  // In-App Notification Banner State (replaces iframe-blocked alert)
  const [banner, setBanner] = useState<{ message: string; type: 'error' | 'success' | 'info' } | null>(null);

  const notifyError = (msg: string) => {
    setBanner({ message: msg, type: 'error' });
  };

  // Fetch System Infrastructure Status (LLM Engine, Nasiko Docker, Embedded DB)
  const fetchSystemStatus = async () => {
    try {
      const res = await fetch('/api/system/status');
      if (res.ok) {
        const data = await res.json();
        setSystemStatus(data);
      }
    } catch (e) {
      console.warn('System status fetch warning:', e);
    }
  };

  // Restore saved state from localStorage if available
  useEffect(() => {
    fetchSystemStatus();
    try {
      const savedProfile = localStorage.getItem('ai_coach_profile');
      const savedCandId = localStorage.getItem('ai_coach_candidate_id');
      if (savedProfile && savedCandId) {
        setCandidateProfile(JSON.parse(savedProfile));
        setCandidateId(savedCandId);
      }
    } catch (e) {
      console.warn('Storage parse error:', e);
    }
  }, []);

  // Agent 1: Resume Reading Agent Handler
  const handleAnalyzeResume = async (resumeText: string, fileName?: string) => {
    setAnalyzingResume(true);
    try {
      const res = await fetch('/api/resume/analyze', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ resume_text: resumeText, file_name: fileName }),
      });

      if (!res.ok) {
        const err = await res.json();
        throw new Error(err.error || 'Resume analysis failed');
      }

      const data = await res.json();
      setCandidateId(data.candidate_id);
      setCandidateProfile(data.candidate_profile);

      // Save to localStorage for convenience
      localStorage.setItem('ai_coach_profile', JSON.stringify(data.candidate_profile));
      localStorage.setItem('ai_coach_candidate_id', data.candidate_id);

      setCurrentView('resume');
    } catch (err: any) {
      notifyError(`Error analyzing resume: ${err.message}`);
    } finally {
      setAnalyzingResume(false);
    }
  };

  // Agent 2: Launch Interview Agent
  const handleStartInterview = async (config: {
    role: string;
    difficulty: 'easy' | 'medium' | 'hard';
    number_of_questions: number;
    duration_minutes: number;
  }) => {
    if (!candidateProfile) return;
    setStartingInterview(true);

    try {
      const res = await fetch('/api/interview/start', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          candidate_id: candidateId,
          candidate_profile: candidateProfile,
          role: config.role,
          difficulty: config.difficulty,
          number_of_questions: config.number_of_questions,
          duration_minutes: config.duration_minutes,
        }),
      });

      if (!res.ok) {
        const err = await res.json();
        throw new Error(err.error || 'Failed to start interview');
      }

      const data = await res.json();
      setSession(data.session);
      setCurrentQuestion(data.question);
      setQuestions([data.question]);
      setAnswers([]);
      setEvaluations([]);
      setReport(null);
      setPlan(null);

      setCurrentView('interview');
    } catch (err: any) {
      notifyError(`Failed to start interview: ${err.message}`);
    } finally {
      setStartingInterview(false);
    }
  };

  // Agent 3 & Agent 2: Submit Answer, Evaluate, and Fetch Next Adaptive Question
  const handleSubmitAnswer = async (answerText: string) => {
    if (!session || !currentQuestion) return;
    setSubmittingAnswer(true);

    try {
      const res = await fetch('/api/interview/answer', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          session_id: session.id,
          question_id: currentQuestion.id,
          answer: answerText,
        }),
      });

      if (!res.ok) {
        const err = await res.json();
        throw new Error(err.error || 'Failed to submit answer');
      }

      const data = await res.json();

      // Record answer and evaluation
      setAnswers((prev) => [...prev, { question_id: currentQuestion.id, answer: answerText }]);
      setEvaluations((prev) => [...prev, data.evaluation]);

      if (data.is_completed) {
        // All questions finished, trigger Agent 4
        await handleGenerateReport(session.id);
      } else if (data.next_question) {
        // Progress to next question
        setCurrentQuestion(data.next_question);
        setQuestions((prev) => [...prev, data.next_question]);
      }
    } catch (err: any) {
      notifyError(`Answer submission error: ${err.message}`);
    } finally {
      setSubmittingAnswer(false);
    }
  };

  // Agent 4: Generate Report
  const handleGenerateReport = async (sessionIdOverride?: string) => {
    const sId = sessionIdOverride || session?.id;
    if (!sId) return;
    setGeneratingReport(true);

    try {
      const res = await fetch('/api/report/generate', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ session_id: sId }),
      });

      if (!res.ok) {
        const err = await res.json();
        throw new Error(err.error || 'Failed to generate report');
      }

      const data = await res.json();
      setReport(data.report);
      setCurrentView('report');
    } catch (err: any) {
      notifyError(`Report generation error: ${err.message}`);
    } finally {
      setGeneratingReport(false);
    }
  };

  // Agent 5: Generate Personalized Preparation Plan
  const handleGeneratePlan = async () => {
    if (!session) return;
    setGeneratingPlan(true);

    try {
      const res = await fetch('/api/preparation/generate', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ session_id: session.id }),
      });

      if (!res.ok) {
        const err = await res.json();
        throw new Error(err.error || 'Failed to generate preparation plan');
      }

      const data = await res.json();
      setPlan(data.plan);
      setCurrentView('plan');
    } catch (err: any) {
      notifyError(`Plan generation error: ${err.message}`);
    } finally {
      setGeneratingPlan(false);
    }
  };

  // Quick Seed Demo Session
  const handleLoadDemoSession = async () => {
    try {
      const res = await fetch('/api/demo/seed', { method: 'POST' });
      if (res.ok) {
        const data = await res.json();
        setCandidateId(data.candidate.id);
        setCandidateProfile(data.candidate.candidate_profile);
        setCurrentView('resume');
      }
    } catch (e) {
      console.error('Failed to seed demo:', e);
    }
  };

  return (
    <div className="min-h-screen bg-slate-950 text-slate-100 flex flex-col font-sans selection:bg-indigo-500 selection:text-white">
      {/* Navigation Header */}
      <Navbar
        currentView={currentView}
        onNavigate={(view) => setCurrentView(view)}
        onOpenTraces={() => setIsTraceOpen(true)}
        onOpenSystemStatus={() => setIsSystemStatusOpen(true)}
        systemStatus={systemStatus}
        candidateName={candidateProfile?.candidate.name}
        sessionActive={session?.status === 'in_progress'}
      />

      {/* In-App Non-blocking Notification Banner */}
      {banner && (
        <div className="max-w-7xl mx-auto px-4 sm:px-6 lg:px-8 pt-4 w-full">
          <div
            className={`p-4 rounded-xl text-xs sm:text-sm flex items-center justify-between border shadow-lg transition-all ${
              banner.type === 'error'
                ? 'bg-rose-950/90 border-rose-800 text-rose-200'
                : 'bg-indigo-950/90 border-indigo-800 text-indigo-200'
            }`}
          >
            <div className="flex items-center space-x-2.5">
              <AlertCircle className="w-4 h-4 text-rose-400 flex-shrink-0" />
              <span>{banner.message}</span>
            </div>
            <button
              onClick={() => setBanner(null)}
              className="text-slate-400 hover:text-white p-1 rounded transition-colors ml-4 flex-shrink-0"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        </div>
      )}

      {/* Main Container */}
      <main className="flex-1 max-w-7xl w-full mx-auto px-4 sm:px-6 lg:px-8 py-8">
        {/* VIEW 1: HOME OVERVIEW & LAUNCHPAD */}
        {currentView === 'home' && (
          <div className="space-y-10">
            {/* Hero */}
            <div className="text-center max-w-3xl mx-auto space-y-4 pt-4">
              <div className="inline-flex items-center space-x-2 px-3 py-1 rounded-full bg-indigo-500/10 border border-indigo-500/20 text-indigo-400 text-xs font-semibold">
                <Sparkles className="w-3.5 h-3.5" />
                <span>Production Multi-Agent Architecture</span>
              </div>
              <h1 className="text-4xl sm:text-5xl font-extrabold tracking-tight text-white leading-tight">
                AI Interview Coach
              </h1>
              <p className="text-slate-400 text-base sm:text-lg leading-relaxed">
                A modular 5-agent technical interview and personalized coaching system orchestrated by{' '}
                <span className="text-indigo-400 font-semibold">Nasiko</span>. Grounded strictly in your verified resume and real-time candidate evaluations.
              </p>

              <div className="flex flex-wrap items-center justify-center gap-4 pt-4">
                <button
                  onClick={() => setCurrentView('resume')}
                  className="px-6 py-3.5 rounded-xl bg-gradient-to-r from-indigo-600 to-cyan-600 hover:from-indigo-500 hover:to-cyan-500 text-white font-semibold text-sm flex items-center space-x-2 shadow-xl shadow-indigo-500/25 transition-all"
                >
                  <span>Start Interview Journey</span>
                  <ArrowRight className="w-4 h-4" />
                </button>

                <button
                  onClick={handleLoadDemoSession}
                  className="px-6 py-3.5 rounded-xl bg-slate-900 hover:bg-slate-850 text-slate-200 text-sm font-medium border border-slate-800 transition-colors flex items-center space-x-2"
                >
                  <Play className="w-4 h-4 text-emerald-400" />
                  <span>Load Java Engineer Demo Profile</span>
                </button>
              </div>

              {/* Infrastructure & Integration Status Bar */}
              <div className="pt-2 flex flex-wrap items-center justify-center gap-2 text-xs">
                <button
                  onClick={() => setIsSystemStatusOpen(true)}
                  className="inline-flex items-center space-x-2 px-3 py-1.5 rounded-lg bg-slate-900/90 border border-slate-800 hover:border-slate-700 text-slate-300 transition-colors"
                >
                  <span className="w-2 h-2 rounded-full bg-amber-400 animate-pulse" />
                  <span>LLM: {systemStatus?.llm.activeProvider === 'bedrock' ? 'Amazon Bedrock' : 'Bedrock / Gemini'}</span>
                </button>
                <button
                  onClick={() => setIsSystemStatusOpen(true)}
                  className="inline-flex items-center space-x-2 px-3 py-1.5 rounded-lg bg-slate-900/90 border border-slate-800 hover:border-slate-700 text-slate-300 transition-colors"
                >
                  <span className={`w-2 h-2 rounded-full ${systemStatus?.nasiko.isDockerReachable ? 'bg-emerald-400' : 'bg-cyan-400'}`} />
                  <span>Nasiko: {systemStatus?.nasiko.isDockerReachable ? 'Docker Connected (:8080)' : 'Docker Probed (:8080)'}</span>
                </button>
                <button
                  onClick={() => setIsSystemStatusOpen(true)}
                  className="inline-flex items-center space-x-2 px-3 py-1.5 rounded-lg bg-slate-900/90 border border-slate-800 hover:border-slate-700 text-slate-300 transition-colors"
                >
                  <span className="w-2 h-2 rounded-full bg-emerald-400" />
                  <span>Database: Embedded (No external DB needed)</span>
                </button>
              </div>
            </div>

            {/* 5-Agent Architecture Pipeline Visualizer */}
            <div className="bg-slate-900/80 border border-slate-800 rounded-3xl p-6 sm:p-8 space-y-6 shadow-2xl">
              <div className="flex flex-col sm:flex-row items-start sm:items-center justify-between gap-2 border-b border-slate-800 pb-4">
                <div className="flex items-center space-x-3">
                  <Layers className="w-5 h-5 text-indigo-400" />
                  <h2 className="text-base font-bold text-white">
                    Sequential 5-Agent Architecture & Contracts
                  </h2>
                </div>
                <span className="text-xs font-mono text-cyan-400 bg-cyan-950/60 border border-cyan-800/50 px-2.5 py-1 rounded-md">
                  Control Layer: Nasiko Orchestrator
                </span>
              </div>

              <div className="grid grid-cols-1 md:grid-cols-5 gap-4">
                {[
                  {
                    step: 1,
                    name: 'Resume Reading Agent',
                    desc: 'Extracts verified profile, skills, projects, and interview topics without hallucinations.',
                    tag: 'Agent 1',
                    icon: FileCheck2,
                    color: 'from-blue-500/20 to-cyan-500/10 border-blue-500/30 text-blue-400',
                  },
                  {
                    step: 2,
                    name: 'Interview Agent',
                    desc: 'Generates adaptive technical questions calibrated to candidate resume and prior answers.',
                    tag: 'Agent 2',
                    icon: Bot,
                    color: 'from-indigo-500/20 to-purple-500/10 border-indigo-500/30 text-indigo-400',
                  },
                  {
                    step: 3,
                    name: 'Evaluation Agent',
                    desc: 'Scores each response objectively across 6 dimensions with feedback and missing concepts.',
                    tag: 'Agent 3',
                    icon: CheckCircle2,
                    color: 'from-emerald-500/20 to-teal-500/10 border-emerald-500/30 text-emerald-400',
                  },
                  {
                    step: 4,
                    name: 'Interview Report Agent',
                    desc: 'Synthesizes aggregate scores, categoric mastery, and cross-question patterns.',
                    tag: 'Agent 4',
                    icon: Award,
                    color: 'from-amber-500/20 to-orange-500/10 border-amber-500/30 text-amber-400',
                  },
                  {
                    step: 5,
                    name: 'Preparation Plan Agent',
                    desc: 'Builds actionable 5-7 day study roadmap, daily concepts, and coding challenges.',
                    tag: 'Agent 5',
                    icon: BookOpen,
                    color: 'from-pink-500/20 to-rose-500/10 border-pink-500/30 text-pink-400',
                  },
                ].map((a) => {
                  const IconComp = a.icon;
                  return (
                    <div
                      key={a.step}
                      className={`rounded-2xl p-4 border bg-gradient-to-b ${a.color} space-y-2 flex flex-col justify-between`}
                    >
                      <div className="space-y-2">
                        <div className="flex items-center justify-between">
                          <span className="text-[10px] font-mono uppercase font-bold tracking-wider opacity-80">
                            {a.tag}
                          </span>
                          <IconComp className="w-4 h-4 opacity-90" />
                        </div>
                        <h3 className="text-xs font-bold text-white">{a.name}</h3>
                        <p className="text-[11px] text-slate-300 leading-relaxed">{a.desc}</p>
                      </div>
                      <div className="pt-2 border-t border-white/5 text-[10px] font-mono text-slate-400">
                        Contract: JSON validated
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>

            {/* Quick Resume Upload Card on Home */}
            <ResumeUploader onAnalyze={handleAnalyzeResume} analyzing={analyzingResume} />
          </div>
        )}

        {/* VIEW 2: RESUME PROFILE & INTERVIEW CONFIG */}
        {currentView === 'resume' && (
          <div>
            {candidateProfile ? (
              <CandidateProfileView
                profile={candidateProfile}
                onStartInterview={handleStartInterview}
                startingInterview={startingInterview}
                onReupload={() => {
                  setCandidateProfile(null);
                  setCandidateId(null);
                }}
              />
            ) : (
              <ResumeUploader onAnalyze={handleAnalyzeResume} analyzing={analyzingResume} />
            )}
          </div>
        )}

        {/* VIEW 3: LIVE ADAPTIVE INTERVIEW */}
        {currentView === 'interview' && session && currentQuestion && (
          <InterviewScreen
            session={session}
            currentQuestion={currentQuestion}
            questions={questions}
            answers={answers}
            evaluations={evaluations}
            onSubmitAnswer={handleSubmitAnswer}
            onFinishInterview={() => handleGenerateReport(session.id)}
            submittingAnswer={submittingAnswer}
            generatingReport={generatingReport}
          />
        )}

        {/* VIEW 4: FINAL INTERVIEW REPORT */}
        {currentView === 'report' && report && (
          <InterviewReportView
            report={report}
            candidateName={candidateProfile?.candidate.name}
            targetRole={session?.target_role}
            onGeneratePlan={handleGeneratePlan}
            generatingPlan={generatingPlan}
            hasExistingPlan={!!plan}
            onViewPlan={() => setCurrentView('plan')}
          />
        )}

        {/* VIEW 5: PERSONALIZED PREPARATION PLAN */}
        {currentView === 'plan' && plan && (
          <PreparationPlanView
            plan={plan}
            candidateName={candidateProfile?.candidate.name}
            onBackToReport={() => setCurrentView('report')}
            onRestartNewInterview={() => {
              setSession(null);
              setCurrentQuestion(null);
              setQuestions([]);
              setAnswers([]);
              setEvaluations([]);
              setReport(null);
              setPlan(null);
              setCurrentView('resume');
            }}
          />
        )}
      </main>

      {/* Nasiko Control Plane Trace Drawer */}
      <NasikoTraceDrawer isOpen={isTraceOpen} onClose={() => setIsTraceOpen(false)} />

      {/* Infrastructure & Architecture Modal */}
      <SystemArchitectureModal
        isOpen={isSystemStatusOpen}
        onClose={() => setIsSystemStatusOpen(false)}
        systemStatus={systemStatus}
        onRefresh={fetchSystemStatus}
      />

      {/* Footer */}
      <footer className="border-t border-slate-800/80 bg-slate-950 py-6 text-center text-xs text-slate-500">
        <div className="max-w-7xl mx-auto px-4 flex flex-col sm:flex-row items-center justify-between gap-2">
          <span>AI Interview Coach · Multi-Agent Technical Career Mentorship System</span>
          <span className="font-mono text-slate-400">
            Orchestration: Nasiko · Agents 1-5 Registered
          </span>
        </div>
      </footer>
    </div>
  );
}
