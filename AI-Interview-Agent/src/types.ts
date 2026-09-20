export interface CandidateProfile {
  candidate: {
    name: string;
    email: string;
    phone: string;
    location: string;
  };
  education: Array<{
    degree: string;
    university: string;
    graduation_year: string;
    cgpa?: string;
  }>;
  experience: Array<{
    company: string;
    role: string;
    duration: string;
    responsibilities: string[];
    technologies: string[];
    domain?: string;
  }>;
  skills: {
    languages: string[];
    backend: string[];
    frontend: string[];
    databases: string[];
    cloud: string[];
    devops: string[];
    frameworks: string[];
    testing: string[];
    other: string[];
  };
  projects: Array<{
    name: string;
    description: string;
    technologies: string[];
    candidate_contribution: string;
    technical_concepts: string[];
    potential_interview_topics: string[];
  }>;
  certifications: string[];
  interview_topics: string[];
  seniority_estimate: string;
  resume_summary: string;
  ambiguities_flagged?: string[];
}

export interface InterviewSession {
  id: string;
  candidate_id: string;
  target_role: string;
  difficulty: 'easy' | 'medium' | 'hard';
  status: 'created' | 'in_progress' | 'completed';
  duration_minutes: number;
  number_of_questions: number;
  current_question_index: number;
  created_at: string;
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
  reason?: string;
  expected_concepts: string[];
  question_order: number;
}

export interface EvaluationScores {
  technical_correctness: number;
  completeness: number;
  depth: number;
  problem_solving: number;
  communication: number;
  practical_understanding: number;
  overall: number;
}

export interface AnswerEvaluation {
  id: string;
  answer_id: string;
  question_id: string;
  session_id: string;
  scores: EvaluationScores;
  strengths: string[];
  weaknesses: string[];
  missing_concepts: string[];
  incorrect_concepts?: string[];
  feedback: string;
  recommended_followup_topics?: string[];
  created_at: string;
}

export interface InterviewReport {
  id: string;
  session_id: string;
  overall_score: number;
  technical_score: number;
  communication_score: number;
  problem_solving_score: number;
  category_scores: Record<string, number>;
  strengths: string[];
  weaknesses: string[];
  topics_to_improve: string[];
  repeated_patterns_detected?: string[];
  interview_summary: string;
  detailed_feedback: string;
  recommended_focus_areas: string[];
  created_at: string;
}

export interface DayStudyPlan {
  day: number;
  topic: string;
  goals: string[];
  concepts: string[];
  practice_tasks: string[];
  estimated_minutes: number;
}

export interface PreparationPlan {
  id: string;
  session_id: string;
  learning_priorities: string[];
  study_plan: DayStudyPlan[];
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

export interface SampleResume {
  id: string;
  name: string;
  role: string;
  summary: string;
  skillsPreview: string[];
  resumeText: string;
}

export interface NasikoTrace {
  id: string;
  agentName: string;
  timestamp: string;
  durationMs: number;
  status: 'success' | 'failed';
  errorMessage?: string;
  error?: string;
  input: unknown;
  output: unknown;
  // Backend contract aliases (NasikoTraceSpan): drawer accepts both shapes.
  startedAt?: string;
  completedAt?: string;
  inputSummary?: unknown;
  outputSummary?: unknown;
  tokens?: {
    promptTokens: number;
    completionTokens: number;
    totalTokens: number;
  };
}

export interface NasikoStats {
  agents: Array<{
    name: string;
    displayName: string;
    version: string;
    responsibility: string;
    healthy: boolean;
    totalExecutions: number;
    successfulExecutions: number;
    failedExecutions: number;
    avgDurationMs: number;
  }>;
  summary: {
    totalCalls: number;
    totalTokens: number;
    avgLatencyMs: number;
    activeAgentsCount: number;
  };
  recentTraces: NasikoTrace[];
}

export interface SystemStatus {
  llm: {
    activeProvider: 'bedrock' | 'gemini' | 'openai' | 'fallback';
    activeModel: string;
    bedrock: {
      configured: boolean;
      authType: 'api_key' | 'iam_keys' | 'none';
      region: string;
      modelId: string;
    };
    gemini: {
      configured: boolean;
      models: string[];
    };
    openai: {
      configured: boolean;
      model: string;
    };
  };
  nasiko: {
    mode: 'docker_nasiko' | 'embedded_fallback';
    targetUrl: string;
    isDockerReachable: boolean;
    isOtelReachable: boolean;
    lastCheckedAt: string;
    registeredAgents: Array<{
      agentName: string;
      displayName: string;
      registered: boolean;
    }>;
    dockerInfo: {
      repo: string;
      command: string;
      defaultHost: string;
      statusSummary: string;
    };
  };
  scraper?: {
    configured: boolean;
    apiKeySet: boolean;
    endpoint: string;
    hasCredits: boolean;
    features: string[];
    mode: 'anakin_api' | 'fallback_ready';
  };
  database: {
    type: string;
    path: string;
    configuredExternalUrl: string;
    requiresExternalUrl: boolean;
    status: string;
    message: string;
  };
}

export interface WebScrapeResult {
  url: string;
  title: string;
  markdown: string;
  textContent: string;
  provider: 'anakin.io' | 'native_fallback';
  meta?: {
    description?: string;
    status?: number;
  };
  latencyMs: number;
}

export interface ScrapedJobPosting {
  title: string;
  company: string;
  location?: string;
  requiredSkills: string[];
  seniority?: string;
  description: string;
  url: string;
  provider: string;
}

export type AppView = 'home' | 'resume' | 'interview' | 'report' | 'plan';
