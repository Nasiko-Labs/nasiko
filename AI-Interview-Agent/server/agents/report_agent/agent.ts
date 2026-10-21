import { AgentExecutionContext, BaseAgent } from '../../nasiko/contracts';
import { llmClient } from '../../llm/client';
import { REPORT_AGENT_SYSTEM_PROMPT } from './prompt';
import {
  ReportAgentInput,
  ReportAgentOutput,
  validateReportAgentInput,
  validateReportAgentOutput,
} from './schema';

export class InterviewReportAgent implements BaseAgent<ReportAgentInput, ReportAgentOutput> {
  name = 'report_agent' as const;
  displayName = 'Interview Report Agent';
  version = '1.0.0';
  responsibility =
    'Aggregates per-question evaluations, extracts cross-cutting behavioral and technical patterns, and generates comprehensive reports';

  validateInput(input: unknown) {
    return validateReportAgentInput(input);
  }

  validateOutput(output: unknown) {
    return validateReportAgentOutput(output);
  }

  async execute(input: ReportAgentInput, _context: AgentExecutionContext) {
    const questionMap = new Map(input.questions.map((q) => [q.question_id, q]));
    const answerMap = new Map(input.answers.map((a) => [a.question_id, a.answer]));

    const questionBreakdown = input.evaluations.map((ev, i) => {
      const q = questionMap.get(ev.question_id) || input.questions[i];
      const ans = answerMap.get(ev.question_id) || '';
      return {
        question_id: ev.question_id,
        topic: q?.topic || 'General',
        category: q?.category || 'Technical',
        difficulty: q?.difficulty || 'medium',
        question: q?.question || '',
        answer: ans,
        scores: ev.scores,
        strengths: ev.strengths,
        weaknesses: ev.weaknesses,
        missing_concepts: ev.missing_concepts,
      };
    });

    const userPrompt = `
Generate the final Interview Performance Report for this candidate.

Candidate:
Name: ${input.candidate_profile.candidate?.name || 'Candidate'}
Seniority: ${input.candidate_profile.seniority_estimate || 'Software Engineer'}

Evaluated Questions & Scores:
${JSON.stringify(questionBreakdown, null, 2)}

Instructions:
1. Calculate overall score (0.0 to 10.0), technical score, communication score, problem solving score.
2. Group and calculate category_scores (e.g. "java", "spring_boot", "databases", "system_design", "security").
3. Detect repeated patterns across questions (e.g., "Candidate consistently struggled on transaction isolation and SQL indices across Q2 and Q4").
4. Identify top strengths and key weaknesses with direct references to the topics.
5. Provide a clear, executive interview summary (2-3 paragraphs) and detailed feedback.

Respond with valid JSON matching:
{
  "overall_score": 7.2,
  "technical_score": 7.5,
  "communication_score": 8.0,
  "problem_solving_score": 7.0,
  "category_scores": {
    "java": 8.0,
    "spring_boot": 7.5,
    "databases": 5.0,
    "system_design": 6.5
  },
  "strengths": ["Demonstrated deep understanding of Spring Boot bean lifecycles"],
  "weaknesses": ["Lacked familiarity with database transaction isolation levels"],
  "topics_to_improve": ["SQL Indexing", "Connection Pools", "Spring Security Filter Chain"],
  "repeated_patterns_detected": ["Security and database persistence showed repeated gaps"],
  "interview_summary": "Comprehensive executive overview...",
  "detailed_feedback": "Detailed technical analysis of performance...",
  "recommended_focus_areas": ["PostgreSQL ACID properties", "JWT stateless authentication"]
}
`;

    const llmResponse = await llmClient.generateStructuredJSON<ReportAgentOutput>(
      REPORT_AGENT_SYSTEM_PROMPT,
      userPrompt,
      0.2
    );

    const data = llmResponse.data;
    const rawOverall = Number(data.overall_score);
    data.overall_score = isNaN(rawOverall) || !isFinite(rawOverall)
      ? 6.0
      : Math.min(10.0, Math.max(0.0, Math.round(rawOverall * 10) / 10));
    data.technical_score = Math.min(10.0, Math.max(0.0, Number(data.technical_score) || data.overall_score));
    data.communication_score = Math.min(10.0, Math.max(0.0, Number(data.communication_score) || data.overall_score));
    data.problem_solving_score = Math.min(10.0, Math.max(0.0, Number(data.problem_solving_score) || data.overall_score));
    data.category_scores = data.category_scores || {};
    data.strengths = Array.isArray(data.strengths) ? data.strengths : [];
    data.weaknesses = Array.isArray(data.weaknesses) ? data.weaknesses : [];
    data.topics_to_improve = Array.isArray(data.topics_to_improve) ? data.topics_to_improve : [];
    data.repeated_patterns_detected = Array.isArray(data.repeated_patterns_detected)
      ? data.repeated_patterns_detected
      : [];
    data.recommended_focus_areas = Array.isArray(data.recommended_focus_areas)
      ? data.recommended_focus_areas
      : [];
    data.interview_summary =
      typeof data.interview_summary === 'string' && data.interview_summary.trim().length > 0
        ? data.interview_summary
        : 'Interview completed. See detailed feedback for per-question analysis.';
    data.detailed_feedback =
      typeof data.detailed_feedback === 'string' && data.detailed_feedback.trim().length > 0
        ? data.detailed_feedback
        : data.interview_summary;

    return {
      data,
      llmLatencyMs: llmResponse.latencyMs,
      tokens: llmResponse.estimatedTokens,
    };
  }
}
