import { nasikoOrchestrator } from '../nasiko/orchestrator';
import { ResumeReadingAgent } from './resume_agent/agent';
import { InterviewAgent } from './interview_agent/agent';
import { EvaluationAgent } from './evaluation_agent/agent';
import { InterviewReportAgent } from './report_agent/agent';
import { PreparationPlanAgent } from './preparation_agent/agent';

export const resumeReadingAgent = new ResumeReadingAgent();
export const interviewAgent = new InterviewAgent();
export const evaluationAgent = new EvaluationAgent();
export const interviewReportAgent = new InterviewReportAgent();
export const preparationPlanAgent = new PreparationPlanAgent();

// Register all agents into the Nasiko Orchestrator Control Plane
export function registerAllAgents() {
  nasikoOrchestrator.registerAgent(resumeReadingAgent);
  nasikoOrchestrator.registerAgent(interviewAgent);
  nasikoOrchestrator.registerAgent(evaluationAgent);
  nasikoOrchestrator.registerAgent(interviewReportAgent);
  nasikoOrchestrator.registerAgent(preparationPlanAgent);
  console.log('[Nasiko] All 5 multi-agent components registered successfully into control plane.');
}
