import { CandidateProfile } from '../resume_agent/schema';

export interface EvaluationAgentInput {
  question: {
    question_id: string;
    question: string;
    topic: string;
    category: string;
    difficulty: 'easy' | 'medium' | 'hard';
    expected_concepts?: string[];
  };
  candidate_answer: string;
  candidate_profile?: Partial<CandidateProfile>;
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

export interface EvaluationAgentOutput {
  question_id: string;
  scores: EvaluationScores;
  correct_concepts: string[];
  incorrect_concepts: string[];
  missing_concepts: string[];
  misconceptions: string[];
  strengths: string[];
  weaknesses: string[];
  feedback: string;
  recommended_followup_topics: string[];
}

export function validateEvaluationAgentInput(input: any): { valid: boolean; error?: string } {
  if (!input || typeof input !== 'object') {
    return { valid: false, error: 'Input must be an object' };
  }
  if (!input.question || !input.question.question) {
    return { valid: false, error: 'question object with question text is required' };
  }
  if (typeof input.candidate_answer !== 'string') {
    return { valid: false, error: 'candidate_answer string is required' };
  }
  return { valid: true };
}

export function validateEvaluationAgentOutput(output: any): { valid: boolean; error?: string } {
  if (!output || typeof output !== 'object') {
    return { valid: false, error: 'Output must be an object' };
  }
  if (!output.scores || typeof output.scores !== 'object') {
    return { valid: false, error: 'scores object is required' };
  }

  const scoreKeys = [
    'technical_correctness',
    'completeness',
    'depth',
    'problem_solving',
    'communication',
    'practical_understanding',
    'overall',
  ];

  for (const key of scoreKeys) {
    const val = output.scores[key];
    if (typeof val !== 'number' || isNaN(val) || !isFinite(val)) {
      return { valid: false, error: `Score dimension '${key}' must be a finite number` };
    }
    if (val < 0.0 || val > 10.0) {
      return { valid: false, error: `Score dimension '${key}' (${val}) must be between 0.0 and 10.0` };
    }
  }

  if (!output.feedback || typeof output.feedback !== 'string') {
    return { valid: false, error: 'feedback text is required' };
  }

  // Ensure arrays
  if (output.strengths && !Array.isArray(output.strengths)) {
    return { valid: false, error: 'strengths must be an array' };
  }
  if (output.weaknesses && !Array.isArray(output.weaknesses)) {
    return { valid: false, error: 'weaknesses must be an array' };
  }

  return { valid: true };
}
