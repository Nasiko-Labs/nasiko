import { CandidateProfile } from '../resume_agent/schema';
import { ReportAgentOutput } from '../report_agent/schema';
import { EvaluationAgentOutput } from '../evaluation_agent/schema';

export interface PreparationPlanInput {
  candidate_profile: Partial<CandidateProfile>;
  interview_report: ReportAgentOutput;
  evaluations: EvaluationAgentOutput[];
}

export interface DayStudyPlan {
  day: number;
  topic: string;
  goals: string[];
  concepts: string[];
  practice_tasks: string[];
  estimated_minutes: number;
}

export interface PreparationPlanOutput {
  learning_priorities: string[];
  study_plan: DayStudyPlan[];
  practice_questions: string[];
  coding_exercises: Array<{
    title: string;
    description: string;
    starter_prompt: string;
    difficulty: 'easy' | 'medium' | 'hard';
  }>;
  revision_topics: string[];
  recommended_next_interview: {
    suggested_role: string;
    focus_topics: string[];
    target_difficulty: string;
  };
}

export function validatePreparationPlanInput(input: any): { valid: boolean; error?: string } {
  if (!input || typeof input !== 'object') {
    return { valid: false, error: 'Input must be an object' };
  }
  if (!input.interview_report) {
    return { valid: false, error: 'interview_report is required' };
  }
  return { valid: true };
}

export function validatePreparationPlanOutput(output: any): { valid: boolean; error?: string } {
  if (!output || typeof output !== 'object') {
    return { valid: false, error: 'Output must be an object' };
  }
  if (!Array.isArray(output.study_plan) || output.study_plan.length === 0) {
    return { valid: false, error: 'study_plan array is required' };
  }

  for (let i = 0; i < output.study_plan.length; i++) {
    const item = output.study_plan[i];
    if (!item || typeof item !== 'object') {
      return { valid: false, error: `study_plan[${i}] must be an object` };
    }
    if (typeof item.day !== 'number' || item.day <= 0) {
      return { valid: false, error: `study_plan[${i}].day must be a positive integer` };
    }
    if (!item.topic || typeof item.topic !== 'string') {
      return { valid: false, error: `study_plan[${i}].topic must be a non-empty string` };
    }
  }

  return { valid: true };
}
