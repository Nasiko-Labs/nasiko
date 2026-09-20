import fs from 'fs';
import path from 'path';

export interface Candidate {
  id: string;
  name: string;
  email: string;
  phone?: string;
  location?: string;
  resume_text: string;
  candidate_profile: any;
  created_at: string;
}

export interface InterviewSession {
  id: string;
  candidate_id: string;
  target_role: string;
  difficulty: 'easy' | 'medium' | 'hard';
  status: 'in_progress' | 'completed';
  duration_minutes: number;
  number_of_questions: number;
  current_question_index: number;
  started_at: string;
  completed_at?: string;
}

export interface InterviewQuestion {
  id: string;
  session_id: string;
  question_id: string;
  question: string;
  topic: string;
  category: string;
  difficulty: 'easy' | 'medium' | 'hard';
  reason: string;
  expected_concepts: string[];
  question_order: number;
  created_at: string;
}

export interface Answer {
  id: string;
  question_id: string;
  session_id: string;
  answer: string;
  submitted_at: string;
}

export interface Evaluation {
  id: string;
  answer_id: string;
  question_id: string;
  session_id: string;
  scores: {
    technical_correctness: number;
    completeness: number;
    depth: number;
    problem_solving: number;
    communication: number;
    practical_understanding: number;
    overall: number;
  };
  strengths: string[];
  weaknesses: string[];
  missing_concepts: string[];
  incorrect_concepts: string[];
  feedback: string;
  recommended_followup_topics: string[];
  created_at: string;
}

export interface InterviewReport {
  id: string;
  session_id: string;
  overall_score: number;
  technical_score?: number;
  communication_score?: number;
  problem_solving_score?: number;
  category_scores: Record<string, number>;
  strengths: string[];
  weaknesses: string[];
  topics_to_improve: string[];
  interview_summary: string;
  detailed_feedback: string;
  recommended_focus_areas: string[];
  created_at: string;
}

export interface PreparationPlan {
  id: string;
  session_id: string;
  learning_priorities: string[];
  study_plan: Array<{
    day: number;
    topic: string;
    goals: string[];
    concepts: string[];
    practice_tasks: string[];
    estimated_minutes: number;
  }>;
  practice_questions: string[];
  coding_exercises: Array<
    | string
    | {
        title: string;
        description: string;
        starter_prompt?: string;
        difficulty?: string;
      }
  >;
  revision_topics: string[];
  recommended_next_interview: {
    suggested_role?: string;
    focus_topics?: string[];
    target_difficulty?: string;
  };
  created_at: string;
}

interface DatabaseSchema {
  candidates: Candidate[];
  sessions: InterviewSession[];
  questions: InterviewQuestion[];
  answers: Answer[];
  evaluations: Evaluation[];
  reports: InterviewReport[];
  plans: PreparationPlan[];
}

export class Database {
  private filePath: string;
  private data: DatabaseSchema;

  constructor(customPath?: string) {
    const dataDir = path.join(process.cwd(), 'data');
    if (!fs.existsSync(dataDir)) {
      fs.mkdirSync(dataDir, { recursive: true });
    }
    this.filePath = customPath || path.join(dataDir, 'interview_coach.json');
    this.data = this.load();
  }

  private load(): DatabaseSchema {
    const emptyDb: DatabaseSchema = {
      candidates: [],
      sessions: [],
      questions: [],
      answers: [],
      evaluations: [],
      reports: [],
      plans: [],
    };

    if (fs.existsSync(this.filePath)) {
      try {
        const raw = fs.readFileSync(this.filePath, 'utf-8');
        return { ...emptyDb, ...JSON.parse(raw) };
      } catch (err) {
        console.error('Error reading database file, initializing clean DB:', err);
        return emptyDb;
      }
    }
    return emptyDb;
  }

  private save(): void {
    try {
      const tempPath = `${this.filePath}.tmp.${Date.now()}_${Math.random().toString(36).substring(2, 6)}`;
      fs.writeFileSync(tempPath, JSON.stringify(this.data, null, 2), 'utf-8');
      fs.renameSync(tempPath, this.filePath);
    } catch (err) {
      console.error('Error saving database atomically:', err);
    }
  }

  // Candidates
  createCandidate(candidate: Omit<Candidate, 'id' | 'created_at'>): Candidate {
    const newCandidate: Candidate = {
      ...candidate,
      id: `cand_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
      created_at: new Date().toISOString(),
    };
    this.data.candidates.push(newCandidate);
    this.save();
    return newCandidate;
  }

  getCandidate(id: string): Candidate | undefined {
    return this.data.candidates.find((c) => c.id === id);
  }

  getAllCandidates(): Candidate[] {
    return this.data.candidates;
  }

  // Sessions
  createSession(session: Omit<InterviewSession, 'id' | 'started_at'>): InterviewSession {
    const newSession: InterviewSession = {
      ...session,
      id: `sess_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
      started_at: new Date().toISOString(),
    };
    this.data.sessions.push(newSession);
    this.save();
    return newSession;
  }

  getSession(id: string): InterviewSession | undefined {
    return this.data.sessions.find((s) => s.id === id);
  }

  updateSession(id: string, updates: Partial<InterviewSession>): InterviewSession | undefined {
    const session = this.getSession(id);
    if (!session) return undefined;
    Object.assign(session, updates);
    this.save();
    return session;
  }

  // Questions
  createQuestion(question: Omit<InterviewQuestion, 'id' | 'created_at'>): InterviewQuestion {
    const newQuestion: InterviewQuestion = {
      ...question,
      id: `q_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
      created_at: new Date().toISOString(),
    };
    this.data.questions.push(newQuestion);
    this.save();
    return newQuestion;
  }

  getQuestionsForSession(sessionId: string): InterviewQuestion[] {
    return this.data.questions
      .filter((q) => q.session_id === sessionId)
      .sort((a, b) => a.question_order - b.question_order);
  }

  getQuestion(id: string): InterviewQuestion | undefined {
    return this.data.questions.find((q) => q.id === id || q.question_id === id);
  }

  // Answers (Idempotent: updates if already exists for session_id + question_id)
  createAnswer(answer: Omit<Answer, 'id' | 'submitted_at'>): Answer {
    const existingIndex = this.data.answers.findIndex(
      (a) => a.session_id === answer.session_id && a.question_id === answer.question_id
    );

    if (existingIndex !== -1) {
      this.data.answers[existingIndex].answer = answer.answer;
      this.data.answers[existingIndex].submitted_at = new Date().toISOString();
      this.save();
      return this.data.answers[existingIndex];
    }

    const newAnswer: Answer = {
      ...answer,
      id: `ans_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
      submitted_at: new Date().toISOString(),
    };
    this.data.answers.push(newAnswer);
    this.save();
    return newAnswer;
  }

  getAnswer(sessionId: string, questionId: string): Answer | undefined {
    return this.data.answers.find(
      (a) => a.session_id === sessionId && a.question_id === questionId
    );
  }

  getAnswersForSession(sessionId: string): Answer[] {
    return this.data.answers.filter((a) => a.session_id === sessionId);
  }

  // Evaluations (Idempotent: updates if already exists for session_id + question_id)
  createEvaluation(evaluation: Omit<Evaluation, 'id' | 'created_at'>): Evaluation {
    const existingIndex = this.data.evaluations.findIndex(
      (e) => e.session_id === evaluation.session_id && e.question_id === evaluation.question_id
    );

    if (existingIndex !== -1) {
      const updated: Evaluation = {
        ...this.data.evaluations[existingIndex],
        ...evaluation,
        created_at: new Date().toISOString(),
      };
      this.data.evaluations[existingIndex] = updated;
      this.save();
      return updated;
    }

    const newEvaluation: Evaluation = {
      ...evaluation,
      id: `eval_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
      created_at: new Date().toISOString(),
    };
    this.data.evaluations.push(newEvaluation);
    this.save();
    return newEvaluation;
  }

  getEvaluation(sessionId: string, questionId: string): Evaluation | undefined {
    return this.data.evaluations.find(
      (e) => e.session_id === sessionId && e.question_id === questionId
    );
  }

  getEvaluationsForSession(sessionId: string): Evaluation[] {
    return this.data.evaluations.filter((e) => e.session_id === sessionId);
  }

  // Reports
  createReport(report: Omit<InterviewReport, 'id' | 'created_at'>): InterviewReport {
    // Replace existing report for same session if present
    this.data.reports = this.data.reports.filter((r) => r.session_id !== report.session_id);
    const newReport: InterviewReport = {
      ...report,
      id: `rep_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
      created_at: new Date().toISOString(),
    };
    this.data.reports.push(newReport);
    this.save();
    return newReport;
  }

  getReportBySessionId(sessionId: string): InterviewReport | undefined {
    return this.data.reports.find((r) => r.session_id === sessionId);
  }

  // Preparation Plans
  createPlan(plan: Omit<PreparationPlan, 'id' | 'created_at'>): PreparationPlan {
    this.data.plans = this.data.plans.filter((p) => p.session_id !== plan.session_id);
    const newPlan: PreparationPlan = {
      ...plan,
      id: `plan_${Date.now()}_${Math.random().toString(36).substring(2, 7)}`,
      created_at: new Date().toISOString(),
    };
    this.data.plans.push(newPlan);
    this.save();
    return newPlan;
  }

  getPlanBySessionId(sessionId: string): PreparationPlan | undefined {
    return this.data.plans.find((p) => p.session_id === sessionId);
  }
}

export const db = new Database();
