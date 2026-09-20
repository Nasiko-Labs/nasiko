import { AgentExecutionContext, BaseAgent } from '../../nasiko/contracts';
import { llmClient } from '../../llm/client';
import { EVALUATION_AGENT_SYSTEM_PROMPT } from './prompt';
import {
  EvaluationAgentInput,
  EvaluationAgentOutput,
  validateEvaluationAgentInput,
  validateEvaluationAgentOutput,
} from './schema';

export class EvaluationAgent implements BaseAgent<EvaluationAgentInput, EvaluationAgentOutput> {
  name = 'evaluation_agent' as const;
  displayName = 'Evaluation Agent';
  version = '1.0.0';
  responsibility =
    'Performs rigorous, objective technical evaluations of candidate answers across 6 core competency dimensions';

  validateInput(input: unknown) {
    return validateEvaluationAgentInput(input);
  }

  validateOutput(output: unknown) {
    return validateEvaluationAgentOutput(output);
  }

  async execute(input: EvaluationAgentInput, _context: AgentExecutionContext) {
    const trimmedAnswer = input.candidate_answer.trim();
    const isEmpty = trimmedAnswer.length === 0;

    const userPrompt = `
Evaluate the following candidate interview answer.

Question Details:
ID: ${input.question.question_id}
Topic: ${input.question.topic}
Category: ${input.question.category}
Difficulty: ${input.question.difficulty}
Question: "${input.question.question}"
Expected Key Concepts: ${JSON.stringify(input.question.expected_concepts || [])}

Candidate's Answer:
"${isEmpty ? '(EMPTY / NO ANSWER PROVIDED)' : input.candidate_answer}"

Candidate Background:
Role: ${input.candidate_profile?.seniority_estimate || 'Software Engineer'}

Score from 0.0 to 10.0 for each rubric item.
Be fair, technically precise, and constructive.
If the candidate says "I don't know" or left it blank, technical correctness must be <= 1.0, completeness <= 1.0, and overall <= 1.0.

Return structured JSON conforming to:
{
  "question_id": "${input.question.question_id}",
  "scores": {
    "technical_correctness": 8.0,
    "completeness": 7.0,
    "depth": 6.5,
    "problem_solving": 7.5,
    "communication": 8.0,
    "practical_understanding": 7.0,
    "overall": 7.3
  },
  "correct_concepts": ["concept stated correctly"],
  "incorrect_concepts": ["any factual mistake"],
  "missing_concepts": ["essential item that was omitted"],
  "misconceptions": ["any wrong mental model"],
  "strengths": ["Clear explanation of X"],
  "weaknesses": ["Did not mention Y"],
  "feedback": "Comprehensive technical feedback explaining the strengths, gaps, and ideal answer.",
  "recommended_followup_topics": ["Suggested revision topic"]
}
`;

    const llmResponse = await llmClient.generateStructuredJSON<EvaluationAgentOutput>(
      EVALUATION_AGENT_SYSTEM_PROMPT,
      userPrompt,
      0.1
    );

    const data = llmResponse.data;
    data.question_id = input.question.question_id;

    // Strict AI Guardrail: Sanitize, clamp, and guarantee score bounds [0.0 - 10.0]
    const clampScore = (val: any, fallback: number = 5.0): number => {
      const n = Number(val);
      if (isNaN(n) || !isFinite(n)) return fallback;
      return Math.round(Math.min(10.0, Math.max(0.0, n)) * 10) / 10;
    };

    const defaultScore = isEmpty ? 1.0 : 5.0;
    const rawScores = data.scores || ({} as any);

    const tc = clampScore(rawScores.technical_correctness, defaultScore);
    const co = clampScore(rawScores.completeness, defaultScore);
    const de = clampScore(rawScores.depth, defaultScore);
    const ps = clampScore(rawScores.problem_solving, defaultScore);
    const cm = clampScore(rawScores.communication, defaultScore);
    const pu = clampScore(rawScores.practical_understanding, defaultScore);
    const calculatedOverall = Math.round(((tc + co + de + ps + cm + pu) / 6) * 10) / 10;
    const ov = clampScore(rawScores.overall, calculatedOverall);

    data.scores = {
      technical_correctness: tc,
      completeness: co,
      depth: de,
      problem_solving: ps,
      communication: cm,
      practical_understanding: pu,
      overall: ov,
    };

    data.feedback = data.feedback || (isEmpty ? 'No answer was provided by the candidate.' : 'Answer evaluated.');
    data.correct_concepts = Array.isArray(data.correct_concepts) ? data.correct_concepts : [];
    data.incorrect_concepts = Array.isArray(data.incorrect_concepts) ? data.incorrect_concepts : [];
    data.missing_concepts = Array.isArray(data.missing_concepts) ? data.missing_concepts : [];
    data.misconceptions = Array.isArray(data.misconceptions) ? data.misconceptions : [];
    data.strengths = Array.isArray(data.strengths) ? data.strengths : [];
    data.weaknesses = Array.isArray(data.weaknesses) ? data.weaknesses : [];
    data.recommended_followup_topics = Array.isArray(data.recommended_followup_topics)
      ? data.recommended_followup_topics
      : [];

    return {
      data,
      llmLatencyMs: llmResponse.latencyMs,
      tokens: llmResponse.estimatedTokens,
    };
  }
}
