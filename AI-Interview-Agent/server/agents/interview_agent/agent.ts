import { AgentExecutionContext, BaseAgent } from '../../nasiko/contracts';
import { llmClient } from '../../llm/client';
import { INTERVIEW_AGENT_SYSTEM_PROMPT } from './prompt';
import {
  InterviewAgentInput,
  InterviewAgentOutput,
  validateInterviewAgentInput,
  validateInterviewAgentOutput,
} from './schema';

export class InterviewAgent implements BaseAgent<InterviewAgentInput, InterviewAgentOutput> {
  name = 'interview_agent' as const;
  displayName = 'Interview Agent';
  version = '1.0.0';
  responsibility =
    'Generates adaptive technical questions calibrated to candidate resume and prior answer performance';

  validateInput(input: unknown) {
    return validateInterviewAgentInput(input);
  }

  validateOutput(output: unknown) {
    return validateInterviewAgentOutput(output);
  }

  async execute(input: InterviewAgentInput, _context: AgentExecutionContext) {
    const history = input.conversation_history || [];
    const questionIndex = history.length + 1;
    const totalQuestions = input.interview_config.number_of_questions || 5;

    // Detect last answer's evaluation to instruct adaptive tuning
    const lastItem = history[history.length - 1];
    let adaptiveGuidance = 'This is the opening question. Begin with a resume-grounded project or experience inquiry.';
    if (lastItem && lastItem.evaluation) {
      const overall = lastItem.evaluation.scores.overall;
      if (overall >= 8.5) {
        adaptiveGuidance = `Candidate performed exceptionally well on previous question (${overall}/10). Increase challenge to 'hard', test edge cases, distributed scaling, internal implementation, or concurrency.`;
      } else if (overall >= 6.5) {
        adaptiveGuidance = `Candidate gave a solid answer (${overall}/10). Progress logically to the next key technical domain or deeper practical scenario at 'medium' difficulty.`;
      } else {
        adaptiveGuidance = `Candidate struggled on '${lastItem.topic}' (${overall}/10). Missing concepts: ${lastItem.evaluation.missing_concepts.join(', ')}. Ask a foundational follow-up or check core fundamentals at 'easy' or 'medium' difficulty.`;
      }
    }

    const topicsCovered = Array.from(new Set(history.map((h) => h.topic)));
    const allStrengths = history.flatMap((h) => h.evaluation?.strengths || []);
    const allWeaknesses = history.flatMap((h) => h.evaluation?.weaknesses || []);

    const userPrompt = `
Generate Question #${questionIndex} of ${totalQuestions} for this technical interview.

Target Role: ${input.interview_config.role}
Target Baseline Difficulty: ${input.interview_config.difficulty}
Current Question Number: ${questionIndex} / ${totalQuestions}

Adaptive Strategy for this Turn:
${adaptiveGuidance}

Candidate Stated Profile:
Name: ${input.candidate_profile.candidate.name}
Seniority: ${input.candidate_profile.seniority_estimate}
Key Skills: ${JSON.stringify(input.candidate_profile.skills)}
Projects: ${JSON.stringify(
      input.candidate_profile.projects.map((p) => ({
        name: p.name,
        tech: p.technologies,
        concepts: p.technical_concepts,
      }))
    )}
Interview Topics Detected in Resume: ${JSON.stringify(input.candidate_profile.interview_topics)}

Conversation History So Far (${history.length} questions completed):
${history
  .map(
    (h, idx) => `
Q${idx + 1} (${h.topic} - ${h.difficulty}): "${h.question}"
Candidate Answer: "${h.answer || '(No answer provided)'}"
Score: ${h.evaluation ? `${h.evaluation.scores.overall}/10` : 'Not evaluated yet'}
Evaluation Summary: ${
      h.evaluation
        ? `Strengths: [${h.evaluation.strengths.join(', ')}] | Weaknesses: [${h.evaluation.weaknesses.join(', ')}]`
        : 'N/A'
    }
`
  )
  .join('\n')}

Produce structured JSON adhering strictly to:
{
  "question_id": "Q${String(questionIndex).padStart(3, '0')}",
  "question": "Clear, precise technical question",
  "category": "e.g. Spring Boot / Databases / System Design / REST APIs / Architecture",
  "difficulty": "easy|medium|hard",
  "topic": "e.g. JWT Authentication / Indexing / Connection Pooling / Microservices",
  "reason": "Why this specific question was chosen based on the candidate profile and adaptive progression",
  "expected_concepts": ["concept1", "concept2", "concept3"]
}
`;

    const llmResponse = await llmClient.generateStructuredJSON<Omit<InterviewAgentOutput, 'interview_state'>>(
      INTERVIEW_AGENT_SYSTEM_PROMPT,
      userPrompt,
      0.3
    );

    const generated = llmResponse.data;
    const resolvedDifficulty =
      (generated as any).difficulty || input.interview_config.difficulty || 'medium';
    const output: InterviewAgentOutput = {
      ...generated,
      question_id: generated.question_id || `Q${String(questionIndex).padStart(3, '0')}`,
      difficulty: resolvedDifficulty,
      interview_state: {
        current_question: questionIndex,
        total_questions: totalQuestions,
        topics_covered: [...topicsCovered, generated.topic].filter(Boolean),
        difficulty_level: resolvedDifficulty,
        strengths_detected: Array.from(new Set(allStrengths)),
        weaknesses_detected: Array.from(new Set(allWeaknesses)),
      },
    };

    return {
      data: output,
      llmLatencyMs: llmResponse.latencyMs,
      tokens: llmResponse.estimatedTokens,
    };
  }
}
