import React, { useState } from 'react';
import {
  MessageSquare,
  Sparkles,
  Send,
  HelpCircle,
  TrendingUp,
  AlertTriangle,
  Lightbulb,
  CheckCircle,
  FileCheck2,
  Mic,
  MicOff,
  ChevronDown,
  ChevronUp,
} from 'lucide-react';
import {
  InterviewSession,
  InterviewQuestion,
  AnswerEvaluation,
} from '../types';

interface InterviewScreenProps {
  session: InterviewSession;
  currentQuestion: InterviewQuestion;
  questions: InterviewQuestion[];
  answers: Array<{ question_id: string; answer: string }>;
  evaluations: AnswerEvaluation[];
  onSubmitAnswer: (answer: string) => Promise<void>;
  onFinishInterview: () => Promise<void>;
  submittingAnswer: boolean;
  generatingReport: boolean;
}

export const InterviewScreen: React.FC<InterviewScreenProps> = ({
  session,
  currentQuestion,
  questions,
  answers,
  evaluations,
  onSubmitAnswer,
  onFinishInterview,
  submittingAnswer,
  generatingReport,
}) => {
  const [answerText, setAnswerText] = useState('');
  const [isRecording, setIsRecording] = useState(false);
  const [showExpected, setShowExpected] = useState(false);
  const [expandedHistoryIndex, setExpandedHistoryIndex] = useState<number | null>(null);

  const currentOrder = currentQuestion?.question_order || questions.length || 1;
  const totalQuestions = session.number_of_questions || 5;
  const progressPercent = Math.min(100, Math.round(((currentOrder - 1) / totalQuestions) * 100));

  // Speech to Text support
  const handleToggleSpeech = () => {
    if (isRecording) {
      setIsRecording(false);
      return;
    }

    const SpeechRecognition =
      (window as any).SpeechRecognition || (window as any).webkitSpeechRecognition;

    if (!SpeechRecognition) {
      alert('Speech recognition is not supported in this browser. Please type your answer.');
      return;
    }

    try {
      const recognition = new SpeechRecognition();
      recognition.continuous = true;
      recognition.interimResults = true;
      recognition.lang = 'en-US';

      recognition.onstart = () => setIsRecording(true);
      recognition.onend = () => setIsRecording(false);
      recognition.onerror = () => setIsRecording(false);

      recognition.onresult = (event: any) => {
        let transcript = '';
        for (let i = event.resultIndex; i < event.results.length; ++i) {
          transcript += event.results[i][0].transcript;
        }
        setAnswerText((prev) => (prev ? `${prev} ${transcript}` : transcript));
      };

      recognition.start();
    } catch (e) {
      console.warn('Speech recognition start failed:', e);
      setIsRecording(false);
    }
  };

  const handleQuickAnswer = (type: 'strong' | 'average' | 'gap') => {
    if (type === 'strong') {
      if (currentQuestion.topic.toLowerCase().includes('security') || currentQuestion.topic.toLowerCase().includes('jwt')) {
        setAnswerText(
          'In Spring Security, the authentication lifecycle is managed through the SecurityFilterChain. We implement a custom filter extending OncePerRequestFilter to extract the Bearer token from the Authorization header. We parse and validate the JWT signature using our secret key, verify claims, and construct an AuthenticationToken that is injected into the SecurityContextHolder for thread-local access.'
        );
      } else if (currentQuestion.topic.toLowerCase().includes('index') || currentQuestion.topic.toLowerCase().includes('database')) {
        setAnswerText(
          'In MySQL InnoDB, secondary indexes store the primary key as the pointer rather than the physical tuple. An index lookup performs a B+tree traversal to locate the leaf node. If all requested columns exist in the index, it is a Covering Index requiring zero disk lookups. Using EXPLAIN FORMAT=JSON, we inspect key, key_len, type=ref vs ALL, and filter conditions to eliminate temporary tables and filesort.'
        );
      } else {
        setAnswerText(
          'In our microservice architecture, we designed modular REST services with clean repository boundaries and connection pooling managed by HikariCP. We enforced transaction isolation using @Transactional and handled distributed state transitions using idempotent event messages and circuit breakers to prevent cascading outages.'
        );
      }
    } else if (type === 'average') {
      setAnswerText(
        'We use Spring Boot for building REST APIs with MySQL database. We write controllers, services, and repositories to do CRUD operations. For security, we use JWT tokens passed in headers. For database optimization, we create indexes on frequently searched columns.'
      );
    } else {
      setAnswerText(
        'I am familiar with the high-level concept, but I have not implemented the low-level internals or production tuning directly in my recent projects.'
      );
    }
  };

  const handleSubmit = async () => {
    if (!answerText.trim()) return;
    const textToSubmit = answerText;
    setAnswerText('');
    await onSubmitAnswer(textToSubmit);
  };

  const latestEvaluation = evaluations[evaluations.length - 1];

  const getDifficultyBadge = (diff: string) => {
    switch (diff?.toLowerCase()) {
      case 'easy':
        return 'bg-emerald-950 text-emerald-400 border-emerald-800';
      case 'hard':
        return 'bg-rose-950 text-rose-400 border-rose-800';
      default:
        return 'bg-amber-950 text-amber-400 border-amber-800';
    }
  };

  return (
    <div className="space-y-8 max-w-5xl mx-auto">
      {/* Session Progress Header */}
      <div className="bg-slate-900 border border-slate-800 rounded-2xl p-5 shadow-lg flex flex-col sm:flex-row items-center justify-between gap-4">
        <div className="w-full sm:w-auto space-y-1">
          <div className="flex items-center space-x-2">
            <span className="text-xs font-semibold uppercase tracking-wider text-indigo-400">
              Live Adaptive Interview
            </span>
            <span className="text-xs text-slate-500">·</span>
            <span className="text-xs text-slate-300 font-medium">{session.target_role}</span>
          </div>
          <div className="flex items-center space-x-3">
            <h1 className="text-lg font-bold text-white">
              Question {currentOrder} of {totalQuestions}
            </h1>
            <span className="text-xs font-mono text-slate-400">({progressPercent}% complete)</span>
          </div>
        </div>

        {/* Progress bar */}
        <div className="w-full sm:w-64 bg-slate-800 h-2.5 rounded-full overflow-hidden">
          <div
            className="bg-gradient-to-r from-indigo-500 to-cyan-400 h-full rounded-full transition-all duration-500"
            style={{ width: `${progressPercent}%` }}
          />
        </div>

        {/* Finish early */}
        {evaluations.length > 0 && (
          <button
            onClick={onFinishInterview}
            disabled={generatingReport}
            className="text-xs px-3.5 py-2 rounded-lg bg-slate-800 hover:bg-slate-700 text-slate-300 hover:text-white border border-slate-700 transition-colors flex items-center space-x-1.5"
          >
            <FileCheck2 className="w-3.5 h-3.5 text-indigo-400" />
            <span>Finish & View Report</span>
          </button>
        )}
      </div>

      {/* Main Question Card (Agent 2 Output) */}
      <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 sm:p-8 space-y-6 shadow-xl relative overflow-hidden">
        <div className="flex flex-wrap items-center justify-between gap-2 border-b border-slate-800 pb-4">
          <div className="flex items-center space-x-2">
            <span className="text-xs font-mono font-bold text-indigo-400 bg-indigo-950/60 px-2 py-0.5 rounded border border-indigo-800">
              {currentQuestion.question_id || `Q${currentOrder}`}
            </span>
            <span className="text-xs font-medium text-slate-300">{currentQuestion.category}</span>
            <span className="text-xs text-slate-600">·</span>
            <span className="text-xs text-slate-400">{currentQuestion.topic}</span>
          </div>

          <div className="flex items-center space-x-2">
            <span
              className={`text-[11px] font-semibold uppercase px-2.5 py-0.5 rounded-full border ${getDifficultyBadge(
                currentQuestion.difficulty
              )}`}
            >
              {currentQuestion.difficulty}
            </span>
          </div>
        </div>

        {/* Question Text */}
        <div className="space-y-3">
          <p className="text-lg sm:text-xl font-medium text-white leading-relaxed tracking-tight">
            {currentQuestion.question}
          </p>

          {/* Rationale & Adaptive Strategy explanation */}
          {currentQuestion.reason && (
            <div className="flex items-start space-x-2 text-xs text-slate-400 bg-slate-950/50 p-3 rounded-xl border border-slate-800/80">
              <Sparkles className="w-4 h-4 text-cyan-400 flex-shrink-0 mt-0.5" />
              <div>
                <span className="font-semibold text-slate-300">Agent 2 Adaptive Reasoning: </span>
                <span>{currentQuestion.reason}</span>
              </div>
            </div>
          )}
        </div>

        {/* Expected Concepts Collapsible (For candidate learning transparency) */}
        {currentQuestion.expected_concepts && currentQuestion.expected_concepts.length > 0 && (
          <div>
            <button
              onClick={() => setShowExpected(!showExpected)}
              className="text-xs text-slate-400 hover:text-slate-200 flex items-center space-x-1"
            >
              <Lightbulb className="w-3.5 h-3.5 text-amber-400" />
              <span>{showExpected ? 'Hide' : 'Reveal'} Expected Concepts & Evaluation Rubric</span>
            </button>
            {showExpected && (
              <div className="mt-2 p-3 rounded-lg bg-amber-950/20 border border-amber-900/40 text-xs text-amber-200 flex flex-wrap gap-1.5">
                <span className="text-amber-400 font-semibold block w-full mb-1">
                  Key concepts evaluated:
                </span>
                {currentQuestion.expected_concepts.map((c) => (
                  <span
                    key={c}
                    className="bg-amber-950/60 border border-amber-800 px-2 py-0.5 rounded text-[11px]"
                  >
                    {c}
                  </span>
                ))}
              </div>
            )}
          </div>
        )}

        {/* Answer Input Area */}
        <div className="space-y-4 pt-4 border-t border-slate-800">
          <div className="flex items-center justify-between">
            <label className="text-xs font-medium text-slate-300 flex items-center space-x-2">
              <MessageSquare className="w-3.5 h-3.5 text-indigo-400" />
              <span>Your Technical Answer</span>
            </label>

            {/* Quick Demo Pre-fills for ease of testing */}
            <div className="flex items-center space-x-2 text-xs">
              <span className="text-slate-500 text-[11px] hidden sm:inline">Fill sample:</span>
              <button
                type="button"
                onClick={() => handleQuickAnswer('strong')}
                className="px-2 py-0.5 rounded text-[11px] bg-emerald-950 text-emerald-300 border border-emerald-800 hover:bg-emerald-900 transition-colors"
                title="Populate a senior-grade technical answer"
              >
                Strong (8-9/10)
              </button>
              <button
                type="button"
                onClick={() => handleQuickAnswer('average')}
                className="px-2 py-0.5 rounded text-[11px] bg-amber-950 text-amber-300 border border-amber-800 hover:bg-amber-900 transition-colors"
                title="Populate an average conceptual answer"
              >
                Average (6-7/10)
              </button>
              <button
                type="button"
                onClick={() => handleQuickAnswer('gap')}
                className="px-2 py-0.5 rounded text-[11px] bg-slate-800 text-slate-300 border border-slate-700 hover:bg-slate-700 transition-colors"
                title="Populate an answer with knowledge gap"
              >
                Gap / Pass
              </button>
            </div>
          </div>

          <textarea
            value={answerText}
            onChange={(e) => setAnswerText(e.target.value)}
            disabled={submittingAnswer}
            placeholder="Type your explanation, architectural trade-offs, internal mechanics, or code patterns here..."
            rows={7}
            className="w-full bg-slate-950 border border-slate-800 rounded-xl p-4 text-xs sm:text-sm text-slate-200 font-mono focus:outline-none focus:border-indigo-500 leading-relaxed shadow-inner"
          />

          <div className="flex flex-col sm:flex-row items-center justify-between gap-3">
            <div className="flex items-center space-x-3 text-xs text-slate-400">
              <span>{answerText.length} characters</span>
              <span>·</span>
              <span>{answerText.trim() ? answerText.trim().split(/\s+/).length : 0} words</span>

              {/* Dictate Toggle */}
              <button
                type="button"
                onClick={handleToggleSpeech}
                className={`p-1.5 rounded-lg border transition-colors flex items-center space-x-1 ${
                  isRecording
                    ? 'bg-rose-950 text-rose-300 border-rose-800 animate-pulse'
                    : 'bg-slate-800 text-slate-400 hover:text-slate-200 border-slate-700'
                }`}
                title="Dictate with microphone"
              >
                {isRecording ? <MicOff className="w-3.5 h-3.5" /> : <Mic className="w-3.5 h-3.5" />}
                <span className="text-[10px]">{isRecording ? 'Listening...' : 'Voice input'}</span>
              </button>
            </div>

            <button
              onClick={handleSubmit}
              disabled={submittingAnswer || !answerText.trim()}
              className="w-full sm:w-auto px-6 py-3 rounded-xl bg-indigo-600 hover:bg-indigo-500 disabled:opacity-40 disabled:cursor-not-allowed text-white font-medium text-xs sm:text-sm flex items-center justify-center space-x-2 shadow-lg shadow-indigo-500/25 transition-all"
            >
              {submittingAnswer ? (
                <>
                  <span className="w-4 h-4 border-2 border-white/30 border-t-white rounded-full animate-spin" />
                  <span>Agent 3 (Evaluation Agent) Grading...</span>
                </>
              ) : (
                <>
                  <span>Submit Answer & Evaluate</span>
                  <Send className="w-3.5 h-3.5" />
                </>
              )}
            </button>
          </div>
        </div>
      </div>

      {/* Latest Evaluation Feedback Card (Agent 3 Output) */}
      {latestEvaluation && (
        <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 shadow-xl space-y-5">
          <div className="flex items-center justify-between border-b border-slate-800 pb-3">
            <div className="flex items-center space-x-2">
              <TrendingUp className="w-4 h-4 text-emerald-400" />
              <h2 className="text-sm font-semibold text-white">
                Agent 3 Evaluation for Previous Question
              </h2>
            </div>
            <div className="flex items-center space-x-2">
              <span className="text-xs text-slate-400">Overall Score:</span>
              <span className="text-base font-bold text-emerald-400 font-mono">
                {latestEvaluation.scores.overall} / 10
              </span>
            </div>
          </div>

          {/* Scores Rubric Grid */}
          <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-6 gap-2 text-center text-xs">
            <div className="bg-slate-950 p-2 rounded-lg border border-slate-800">
              <div className="text-[10px] text-slate-400 truncate">Correctness</div>
              <div className="font-bold text-slate-200 mt-0.5">
                {latestEvaluation.scores.technical_correctness}/10
              </div>
            </div>
            <div className="bg-slate-950 p-2 rounded-lg border border-slate-800">
              <div className="text-[10px] text-slate-400 truncate">Completeness</div>
              <div className="font-bold text-slate-200 mt-0.5">
                {latestEvaluation.scores.completeness}/10
              </div>
            </div>
            <div className="bg-slate-950 p-2 rounded-lg border border-slate-800">
              <div className="text-[10px] text-slate-400 truncate">Depth</div>
              <div className="font-bold text-slate-200 mt-0.5">
                {latestEvaluation.scores.depth}/10
              </div>
            </div>
            <div className="bg-slate-950 p-2 rounded-lg border border-slate-800">
              <div className="text-[10px] text-slate-400 truncate">Problem Solving</div>
              <div className="font-bold text-slate-200 mt-0.5">
                {latestEvaluation.scores.problem_solving}/10
              </div>
            </div>
            <div className="bg-slate-950 p-2 rounded-lg border border-slate-800">
              <div className="text-[10px] text-slate-400 truncate">Communication</div>
              <div className="font-bold text-slate-200 mt-0.5">
                {latestEvaluation.scores.communication}/10
              </div>
            </div>
            <div className="bg-slate-950 p-2 rounded-lg border border-slate-800">
              <div className="text-[10px] text-slate-400 truncate">Practical</div>
              <div className="font-bold text-slate-200 mt-0.5">
                {latestEvaluation.scores.practical_understanding}/10
              </div>
            </div>
          </div>

          {/* Feedback & Strengths / Weaknesses */}
          <p className="text-xs sm:text-sm text-slate-300 leading-relaxed bg-slate-950/40 p-4 rounded-xl border border-slate-800">
            {latestEvaluation.feedback}
          </p>

          <div className="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs">
            {latestEvaluation.strengths && latestEvaluation.strengths.length > 0 && (
              <div className="bg-emerald-950/20 border border-emerald-900/40 rounded-xl p-3 space-y-1.5">
                <div className="flex items-center space-x-1.5 text-emerald-400 font-semibold">
                  <CheckCircle className="w-3.5 h-3.5" />
                  <span>Demonstrated Strengths</span>
                </div>
                <ul className="list-disc list-inside text-emerald-200/90 space-y-1">
                  {latestEvaluation.strengths.map((s, idx) => (
                    <li key={idx}>{s}</li>
                  ))}
                </ul>
              </div>
            )}

            {latestEvaluation.weaknesses && latestEvaluation.weaknesses.length > 0 && (
              <div className="bg-amber-950/20 border border-amber-900/40 rounded-xl p-3 space-y-1.5">
                <div className="flex items-center space-x-1.5 text-amber-400 font-semibold">
                  <AlertTriangle className="w-3.5 h-3.5" />
                  <span>Gaps & Missing Nuances</span>
                </div>
                <ul className="list-disc list-inside text-amber-200/90 space-y-1">
                  {latestEvaluation.weaknesses.map((w, idx) => (
                    <li key={idx}>{w}</li>
                  ))}
                </ul>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Previous Questions & Evaluations Accordion */}
      {questions.length > 1 && (
        <div className="bg-slate-900 border border-slate-800 rounded-2xl p-6 shadow-md space-y-3">
          <h3 className="text-xs font-semibold uppercase tracking-wider text-slate-400">
            Interview Q&A History ({questions.length - 1} completed)
          </h3>

          <div className="space-y-2">
            {questions.slice(0, -1).map((q, idx) => {
              const answerObj = answers.find((a) => a.question_id === q.id);
              const evalObj = evaluations.find((e) => e.question_id === q.id);
              const isExpanded = expandedHistoryIndex === idx;

              return (
                <div
                  key={q.id}
                  className="border border-slate-800 rounded-xl overflow-hidden bg-slate-950/40"
                >
                  <button
                    onClick={() => setExpandedHistoryIndex(isExpanded ? null : idx)}
                    className="w-full p-3.5 text-left flex items-center justify-between hover:bg-slate-900/60 transition-colors"
                  >
                    <div className="flex items-center space-x-3 text-xs">
                      <span className="font-mono font-bold text-indigo-400">{q.question_id}</span>
                      <span className="text-slate-200 font-medium truncate max-w-md">
                        {q.question}
                      </span>
                    </div>
                    <div className="flex items-center space-x-3">
                      {evalObj && (
                        <span className="text-xs font-mono font-bold text-emerald-400">
                          {evalObj.scores.overall}/10
                        </span>
                      )}
                      {isExpanded ? (
                        <ChevronUp className="w-4 h-4 text-slate-400" />
                      ) : (
                        <ChevronDown className="w-4 h-4 text-slate-400" />
                      )}
                    </div>
                  </button>

                  {isExpanded && (
                    <div className="p-4 border-t border-slate-800 space-y-3 text-xs bg-slate-900/40">
                      <div>
                        <span className="font-semibold text-slate-400 block mb-1">Your Answer:</span>
                        <p className="text-slate-300 font-mono bg-slate-950 p-2.5 rounded border border-slate-800">
                          {answerObj?.answer || '(No answer recorded)'}
                        </p>
                      </div>

                      {evalObj && (
                        <div>
                          <span className="font-semibold text-slate-400 block mb-1">
                            Agent 3 Feedback:
                          </span>
                          <p className="text-slate-300 bg-slate-950/80 p-2.5 rounded border border-slate-800">
                            {evalObj.feedback}
                          </p>
                        </div>
                      )}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
};
