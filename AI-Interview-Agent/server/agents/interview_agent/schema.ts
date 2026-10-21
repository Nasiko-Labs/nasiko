import { CandidateProfile } from '../resume_agent/schema';

export interface InterviewHistoryItem {
  question_id: string;
  question: string;
  topic: string;
  category: string;
  difficulty: 'easy' | 'medium' | 'hard';
  answer?: string;
  evaluation?: {
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
  };
}

export interface InterviewAgentInput {
  candidate_profile: CandidateProfile;
  interview_config: {
    role: string;
    difficulty: 'easy' | 'medium' | 'hard';
    duration_minutes: number;
    number_of_questions: number;
  };
  conversation_history: InterviewHistoryItem[];
}

export interface InterviewAgentOutput {
  question_id: string;
  question: string;
  category: string;
  difficulty: 'easy' | 'medium' | 'hard';
  topic: string;
  reason: string;
  expected_concepts: string[];
  interview_state: {
    current_question: number;
    total_questions: number;
    topics_covered: string[];
    difficulty_level: string;
    strengths_detected: string[];
    weaknesses_detected: string[];
  };
}

export function validateInterviewAgentInput(input: any): { valid: boolean; error?: string } {
  if (!input || typeof input !== 'object') {
    return { valid: false, error: 'Input must be an object' };
  }
  if (!input.candidate_profile || typeof input.candidate_profile !== 'object') {
    return { valid: false, error: 'candidate_profile is required' };
  }
  if (!input.interview_config || !input.interview_config.role) {
    return { valid: false, error: 'interview_config.role is required' };
  }
  return { valid: true };
}

export function validateInterviewAgentOutput(output: any): { valid: boolean; error?: string } {
  if (!output || typeof output !== 'object') {
    return { valid: false, error: 'Output must be an object' };
  }
  if (!output.question || typeof output.question !== 'string') {
    return { valid: false, error: 'Generated question text is required' };
  }
  if (!output.topic || !output.category) {
    return { valid: false, error: 'Topic and category are required' };
  }
  return { valid: true };
}
