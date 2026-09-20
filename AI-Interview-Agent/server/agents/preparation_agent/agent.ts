import { AgentExecutionContext, BaseAgent } from '../../nasiko/contracts';
import { llmClient } from '../../llm/client';
import { PREPARATION_AGENT_SYSTEM_PROMPT } from './prompt';
import {
  PreparationPlanInput,
  PreparationPlanOutput,
  validatePreparationPlanInput,
  validatePreparationPlanOutput,
} from './schema';

export class PreparationPlanAgent implements BaseAgent<PreparationPlanInput, PreparationPlanOutput> {
  name = 'preparation_agent' as const;
  displayName = 'Personalized Preparation Plan Agent';
  version = '1.0.0';
  responsibility =
    'Generates hyper-specific daily roadmaps, coding exercises, and targeted revision schedules from performance evaluations';

  validateInput(input: unknown) {
    return validatePreparationPlanInput(input);
  }

  validateOutput(output: unknown) {
    return validatePreparationPlanOutput(output);
  }

  async execute(input: PreparationPlanInput, _context: AgentExecutionContext) {
    const report = input.interview_report;
    const allMissingConcepts = Array.from(
      new Set(input.evaluations.flatMap((e) => e.missing_concepts || []))
    );
    const allIncorrectConcepts = Array.from(
      new Set(input.evaluations.flatMap((e) => e.incorrect_concepts || []))
    );

    const userPrompt = `
Generate a Personalized Preparation Plan based on this candidate's interview results.

Candidate Target Profile:
Candidate Name: ${input.candidate_profile.candidate?.name || 'Candidate'}
Seniority: ${input.candidate_profile.seniority_estimate || 'Software Engineer'}

Report Summary:
Overall Score: ${report.overall_score}/10
Category Breakdown: ${JSON.stringify(report.category_scores)}
Weaknesses Identified: ${JSON.stringify(report.weaknesses)}
Topics to Improve: ${JSON.stringify(report.topics_to_improve)}
Patterns Detected: ${JSON.stringify(report.repeated_patterns_detected || [])}
Missing Technical Concepts: ${JSON.stringify(allMissingConcepts)}
Incorrect/Misconceptions: ${JSON.stringify(allIncorrectConcepts)}

Requirements:
1. learning_priorities: Ranked list of top 3-5 learning priorities.
2. study_plan: A structured 5-day or 7-day study roadmap where every day contains:
   - day: number (1..5 or 7)
   - topic: concise name
   - goals: 1-2 bullet points with actionable objectives
   - concepts: list of exact classes/interfaces/patterns (e.g., SecurityFilterChain, OncePerRequestFilter, WAL, B-Tree indexing)
   - practice_tasks: hands-on coding or implementation tasks
   - estimated_minutes: 45 to 90 minutes
3. practice_questions: 4-6 targeted technical interview questions to practice next.
4. coding_exercises: 2-3 coding challenge descriptions with starter prompts.
5. revision_topics: 3-5 high-yield revision topics.
6. recommended_next_interview: guidance on what to target in the next interview.

Produce clean, valid JSON matching:
{
  "learning_priorities": [
    "Priority 1: Spring Security & JWT Filter Chain",
    "Priority 2: Database Indexing & Query Plans",
    "Priority 3: Microservice Resiliency & Circuit Breakers"
  ],
  "study_plan": [
    {
      "day": 1,
      "topic": "Spring Security Architecture & Filter Chain",
      "goals": ["Understand the delegation chain from DelegatingFilterProxy to SecurityFilterChain"],
      "concepts": ["SecurityFilterChain", "OncePerRequestFilter", "SecurityContextPersistenceFilter"],
      "practice_tasks": ["Build a standalone Spring Boot 3 app with a custom authorization filter"],
      "estimated_minutes": 60
    }
  ],
  "practice_questions": [
    "How does SecurityContextHolder store authentication in thread-local storage?"
  ],
  "coding_exercises": [
    {
      "title": "JWT Authentication Filter Implementation",
      "description": "Implement an authentication filter that validates a Bearer token and sets SecurityContextHolder",
      "starter_prompt": "public class JwtAuthenticationFilter extends OncePerRequestFilter { ... }",
      "difficulty": "medium"
    }
  ],
  "revision_topics": ["Spring Security", "B-Tree Indexes vs Hash Indexes"],
  "recommended_next_interview": {
    "suggested_role": "Senior Java Backend Engineer",
    "focus_topics": ["Spring Security", "Distributed Transactions"],
    "target_difficulty": "medium-hard"
  }
}
`;

    const llmResponse = await llmClient.generateStructuredJSON<PreparationPlanOutput>(
      PREPARATION_AGENT_SYSTEM_PROMPT,
      userPrompt,
      0.2
    );

    const data = llmResponse.data;
    data.learning_priorities = data.learning_priorities || [];
    data.study_plan = data.study_plan || [];
    data.practice_questions = data.practice_questions || [];
    data.coding_exercises = data.coding_exercises || [];
    data.revision_topics = data.revision_topics || [];
    data.recommended_next_interview = data.recommended_next_interview || {
      suggested_role: 'Software Engineer',
      focus_topics: [],
      target_difficulty: 'medium',
    };

    return {
      data,
      llmLatencyMs: llmResponse.latencyMs,
      tokens: llmResponse.estimatedTokens,
    };
  }
}
