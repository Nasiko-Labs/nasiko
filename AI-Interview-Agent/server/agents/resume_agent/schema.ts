export interface ResumeAgentInput {
  resume_text: string;
  file_name?: string;
}

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

export function validateResumeAgentInput(input: any): { valid: boolean; error?: string } {
  if (!input || typeof input !== 'object') {
    return { valid: false, error: 'Input must be an object' };
  }
  if (!input.resume_text || typeof input.resume_text !== 'string') {
    return { valid: false, error: 'resume_text is required and must be a string' };
  }
  if (input.resume_text.trim().length < 30) {
    return { valid: false, error: 'resume_text is too short to be a valid resume' };
  }
  return { valid: true };
}

export function validateResumeAgentOutput(output: any): { valid: boolean; error?: string } {
  if (!output || typeof output !== 'object') {
    return { valid: false, error: 'Output must be a valid object' };
  }
  if (!output.candidate || typeof output.candidate.name !== 'string') {
    return { valid: false, error: 'candidate.name is missing in output profile' };
  }
  if (!output.skills || typeof output.skills !== 'object') {
    return { valid: false, error: 'skills categorized object is missing' };
  }
  return { valid: true };
}
