import { CandidateProfile } from '../resume_agent/schema';
import { EvaluationAgentOutput } from '../evaluation_agent/schema';

export interface ReportAgentInput {
  candidate_profile: Partial<CandidateProfile>;
  questions: Array<{
    id: string;
    question_id: string;
    question: string;
    topic: string;
    category: string;
    difficulty: string;
  }>;
  answers: Array<{
    question_id: string;
    answer: string;
  }>;
  evaluations: EvaluationAgentOutput[];
}

export interface ReportAgentOutput {
  overall_score: number;
  technical_score: number;
  communication_score: number;
  problem_solving_score: number;
  category_scores: Record<string, number>;
  strengths: string[];
  weaknesses: string[];
  topics_to_improve: string[];
  repeated_patterns_detected: string[];
  interview_summary: string;
  detailed_feedback: string;
  recommended_focus_areas: string[];
}

export function validateReportAgentInput(input: any): { valid: boolean; error?: string } {
  if (!input || typeof input !== 'object') {
    return { valid: false, error: 'Input must be an object' };
  }
  if (!Array.isArray(input.evaluations) || input.evaluations.length === 0) {
    return { valid: false, error: 'At least one evaluation is required to generate a report' };
  }
  return { valid: true };
}

export function validateReportAgentOutput(output: any): { valid: boolean; error?: string } {
  if (!output || typeof output !== 'object') {
    return { valid: false, error: 'Output must be an object' };
  }
  if (typeof output.overall_score !== 'number' || isNaN(output.overall_score) || !isFinite(output.overall_score)) {
    return { valid: false, error: 'overall_score finite number is required' };
  }
  if (output.overall_score < 0.0 || output.overall_score > 10.0) {
    return { valid: false, error: `overall_score (${output.overall_score}) must be between 0.0 and 10.0` };
  }
  if (!output.category_scores || typeof output.category_scores !== 'object') {
    return { valid: false, error: 'category_scores map is required' };
  }
  return { valid: true };
}
