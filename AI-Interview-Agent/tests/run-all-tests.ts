/**
 * Comprehensive Automated Quality & Testing Suite
 * Validates schemas, state machine, idempotency, score bounds,
 * adversarial payloads, fallbacks, and multi-agent orchestration.
 */

import { validateResumeAgentInput, validateResumeAgentOutput } from '../server/agents/resume_agent/schema';
import { validateInterviewAgentInput, validateInterviewAgentOutput } from '../server/agents/interview_agent/schema';
import { validateEvaluationAgentInput, validateEvaluationAgentOutput } from '../server/agents/evaluation_agent/schema';
import { validateReportAgentInput, validateReportAgentOutput } from '../server/agents/report_agent/schema';
import { validatePreparationPlanInput, validatePreparationPlanOutput } from '../server/agents/preparation_agent/schema';
import { Database } from '../server/db/database';
import { nasikoOrchestrator } from '../server/nasiko/orchestrator';
import { nasikoObservability } from '../server/nasiko/observability';
import { nasikoClientBridge } from '../server/nasiko/clientBridge';
import { llmClient } from '../server/llm/client';
import { generateHeuristicFallback } from '../server/llm/heuristicFallback';
import { anakinScraperClient } from '../server/scraper/anakinClient';
import { registerAllAgents } from '../server/agents/index';
import * as path from 'path';
import * as fs from 'fs';

interface TestResult {
  suite: string;
  name: string;
  passed: boolean;
  error?: string;
  durationMs: number;
}

const results: TestResult[] = [];

function assert(condition: boolean, message: string) {
  if (!condition) {
    throw new Error(message);
  }
}

function runTest(suite: string, name: string, fn: () => void | Promise<void>) {
  const start = Date.now();
  try {
    const res = fn();
    if (res instanceof Promise) {
      return res
        .then(() => {
          results.push({ suite, name, passed: true, durationMs: Date.now() - start });
        })
        .catch((err: any) => {
          results.push({ suite, name, passed: false, error: err?.message || String(err), durationMs: Date.now() - start });
        });
    }
    results.push({ suite, name, passed: true, durationMs: Date.now() - start });
    return Promise.resolve();
  } catch (err: any) {
    results.push({ suite, name, passed: false, error: err?.message || String(err), durationMs: Date.now() - start });
    return Promise.resolve();
  }
}

export async function runFullQualitySuite() {
  console.log('===========================================================');
  console.log('⚡ RUNNING AUTOMATED CODE QUALITY & VERIFICATION SUITE');
  console.log('===========================================================\n');

  // Register all agents
  registerAllAgents();

  // -------------------------------------------------------------
  // SUITE 1: SCHEMA VALIDATION & INTEGRITY
  // -------------------------------------------------------------
  await runTest('1. Schema Validation', 'Resume input validator accepts valid resume', () => {
    const valid = validateResumeAgentInput({ resume_text: 'Experienced Software Engineer with 5 years in TypeScript and Node.js.' });
    assert(valid.valid === true, 'Expected valid resume input to pass');
  });

  await runTest('1. Schema Validation', 'Resume input validator rejects short or empty resume', () => {
    const invalidShort = validateResumeAgentInput({ resume_text: 'short' });
    assert(invalidShort.valid === false, 'Expected short resume to fail');
    const invalidEmpty = validateResumeAgentInput({});
    assert(invalidEmpty.valid === false, 'Expected empty resume to fail');
  });

  await runTest('1. Schema Validation', 'Resume output validator checks required fields', () => {
    const invalid = validateResumeAgentOutput({ candidate: {} });
    assert(invalid.valid === false, 'Expected missing candidate.name to fail');
    const valid = validateResumeAgentOutput({ candidate: { name: 'Alice' }, skills: {} });
    assert(valid.valid === true, 'Expected candidate with name & skills to pass');
  });

  await runTest('1. Schema Validation', 'Interview output validator requires question text', () => {
    const invalid = validateInterviewAgentOutput({ topic: 'Database' });
    assert(invalid.valid === false, 'Expected missing question text to fail');
    const valid = validateInterviewAgentOutput({ question: 'What is ACID?', topic: 'DB', category: 'Backend' });
    assert(valid.valid === true, 'Expected complete question output to pass');
  });

  // -------------------------------------------------------------
  // SUITE 2: SCORE BOUNDS & AI GUARDRAILS (Section 31)
  // -------------------------------------------------------------
  await runTest('2. Score Guardrails', 'Evaluation validator rejects score > 10.0', () => {
    const invalid = validateEvaluationAgentOutput({
      scores: {
        technical_correctness: 12.0,
        completeness: 8.0,
        depth: 7.0,
        problem_solving: 8.0,
        communication: 9.0,
        practical_understanding: 8.0,
        overall: 8.0,
      },
      feedback: 'Good answer',
    });
    assert(invalid.valid === false, 'Expected score 12.0 to be rejected');
    assert(invalid.error?.includes('between 0.0 and 10.0') || false, 'Expected score boundary error');
  });

  await runTest('2. Score Guardrails', 'Evaluation validator rejects score < 0.0', () => {
    const invalid = validateEvaluationAgentOutput({
      scores: {
        technical_correctness: -1.0,
        completeness: 8.0,
        depth: 7.0,
        problem_solving: 8.0,
        communication: 9.0,
        practical_understanding: 8.0,
        overall: 8.0,
      },
      feedback: 'Good answer',
    });
    assert(invalid.valid === false, 'Expected negative score to be rejected');
  });

  await runTest('2. Score Guardrails', 'Evaluation validator rejects NaN / non-number scores', () => {
    const invalid = validateEvaluationAgentOutput({
      scores: {
        technical_correctness: NaN,
        completeness: 8.0,
        depth: 7.0,
        problem_solving: 8.0,
        communication: 9.0,
        practical_understanding: 8.0,
        overall: 8.0,
      },
      feedback: 'Good answer',
    });
    assert(invalid.valid === false, 'Expected NaN score to be rejected');
  });

  await runTest('2. Score Guardrails', 'Report validator rejects overall_score > 10.0', () => {
    const invalid = validateReportAgentOutput({
      overall_score: 95.0,
      category_scores: { backend: 8 },
    });
    assert(invalid.valid === false, 'Expected report score > 10 to be rejected');
  });

  await runTest('2. Score Guardrails', 'Preparation Plan validator enforces positive days', () => {
    const invalid = validatePreparationPlanOutput({
      study_plan: [{ day: -1, topic: 'Algorithms' }],
    });
    assert(invalid.valid === false, 'Expected negative day to be rejected');
    const valid = validatePreparationPlanOutput({
      study_plan: [{ day: 1, topic: 'Algorithms', estimated_minutes: 60 }],
    });
    assert(valid.valid === true, 'Expected valid study plan to pass');
  });

  // -------------------------------------------------------------
  // SUITE 3: DATABASE PERSISTENCE, ATOMICITY & IDEMPOTENCY
  // -------------------------------------------------------------
  const testDbPath = path.resolve(process.cwd(), 'data', 'test_interview_coach.json');
  if (fs.existsSync(testDbPath)) fs.unlinkSync(testDbPath);
  const testDb = new Database(testDbPath);

  await runTest('3. Persistence & Idempotency', 'Candidate and Session creation', () => {
    const candidate = testDb.createCandidate({
      name: 'Test Candidate',
      email: 'test@candidate.com',
      resume_text: 'Sample software engineer resume',
      candidate_profile: {} as any,
    });
    assert(Boolean(candidate.id), 'Expected candidate ID generated');

    const session = testDb.createSession({
      candidate_id: candidate.id,
      target_role: 'Full Stack Engineer',
      difficulty: 'medium',
      duration_minutes: 30,
      number_of_questions: 3,
      status: 'in_progress',
      current_question_index: 0,
    });
    assert(session.status === 'in_progress', 'Expected session in_progress');
    assert(session.target_role === 'Full Stack Engineer', 'Expected target_role persisted (regression: role -> target_role)');
  });

  await runTest('3. Persistence & Idempotency', 'Answer submission is idempotent (updates existing instead of duplicating)', () => {
    const q = testDb.createQuestion({
      session_id: 'sess_test_1',
      question_id: 'q_test_1',
      question: 'Explain indexing in databases.',
      topic: 'Databases',
      category: 'Backend',
      difficulty: 'medium',
      reason: 'Core database fundamentals probe',
      expected_concepts: ['B-Tree', 'Scan cost'],
      question_order: 1,
    });

    // First submission
    const a1 = testDb.createAnswer({
      question_id: q.id,
      session_id: 'sess_test_1',
      answer: 'Indexes use B-trees to speed up queries.',
    });

    // Duplicate submission (simulating double click or network retry)
    const a2 = testDb.createAnswer({
      question_id: q.id,
      session_id: 'sess_test_1',
      answer: 'Indexes use B-trees to speed up lookup times.',
    });

    const answers = testDb.getAnswersForSession('sess_test_1');
    assert(answers.length === 1, `Expected exactly 1 answer record, but found ${answers.length}`);
    assert(answers[0].answer === 'Indexes use B-trees to speed up lookup times.', 'Expected updated answer content');
  });

  await runTest('3. Persistence & Idempotency', 'Evaluation creation is idempotent', () => {
    const eval1 = testDb.createEvaluation({
      answer_id: 'ans_1',
      question_id: 'q_test_1',
      session_id: 'sess_test_1',
      scores: {
        technical_correctness: 8,
        completeness: 7,
        depth: 7,
        problem_solving: 8,
        communication: 8,
        practical_understanding: 8,
        overall: 7.7,
      },
      feedback: 'Good initial answer',
      strengths: ['Clear terminology'],
      weaknesses: [],
      missing_concepts: [],
      incorrect_concepts: [],
      recommended_followup_topics: [],
    });

    // Duplicate evaluation call
    const eval2 = testDb.createEvaluation({
      answer_id: 'ans_1',
      question_id: 'q_test_1',
      session_id: 'sess_test_1',
      scores: {
        technical_correctness: 9,
        completeness: 8,
        depth: 8,
        problem_solving: 9,
        communication: 9,
        practical_understanding: 9,
        overall: 8.7,
      },
      feedback: 'Refined evaluation',
      strengths: ['Clear terminology'],
      weaknesses: [],
      missing_concepts: [],
      incorrect_concepts: [],
      recommended_followup_topics: [],
    });

    const evaluations = testDb.getEvaluationsForSession('sess_test_1');
    assert(evaluations.length === 1, `Expected exactly 1 evaluation record, but found ${evaluations.length}`);
    assert(evaluations[0].scores.overall === 8.7, 'Expected updated evaluation content');
  });

  // Cleanup test database
  if (fs.existsSync(testDbPath)) fs.unlinkSync(testDbPath);

  // -------------------------------------------------------------
  // SUITE 4: MULTI-AGENT ORCHESTRATION PIPELINE
  // -------------------------------------------------------------
  await runTest('4. Orchestration Pipeline', 'Agent 1 (ResumeReadingAgent) synthesizes profile', async () => {
    const res = await nasikoOrchestrator.execute<any, any>('resume_agent', {
      resume_text: `Alice Smith
Senior Backend Engineer
5 years building microservices with Node.js, Express, PostgreSQL, Redis, and AWS.
Graduated Computer Science from UC Berkeley in 2019.`,
    });
    assert(Boolean(res.data), 'Expected agent output data');
    assert(Boolean(res.data.candidate?.name), 'Expected candidate name synthesized');
    assert(Array.isArray(res.data.skills?.backend), 'Expected backend skills categorized');
    assert(Boolean(res.traceId), 'Expected trace ID recorded');
  });

  await runTest('4. Orchestration Pipeline', 'Agent 2 (InterviewAgent) generates tailored question', async () => {
    const res = await nasikoOrchestrator.execute<any, any>('interview_agent', {
      candidate_profile: {
        candidate: { name: 'Alice Smith', email: 'alice@example.com', phone: '', location: '' },
        skills: { backend: ['Node.js', 'PostgreSQL'], languages: ['TypeScript'], frontend: [], databases: ['PostgreSQL'], cloud: ['AWS'], devops: [], frameworks: [], testing: [], other: [] },
        experience: [],
        education: [],
        projects: [],
        certifications: [],
        interview_topics: ['PostgreSQL Indexing', 'Node.js Event Loop'],
        seniority_estimate: 'Senior',
        resume_summary: 'Experienced Backend Engineer',
      },
      interview_config: {
        role: 'Senior Backend Engineer',
        difficulty: 'medium',
        duration_minutes: 30,
        number_of_questions: 3,
      },
      conversation_history: [],
    });
    assert(Boolean(res.data.question), 'Expected question text');
    assert(Boolean(res.data.topic), 'Expected topic assigned');
    assert(Boolean(res.data.interview_state), 'Expected interview state tracking');
  });

  await runTest('4. Orchestration Pipeline', 'Agent 3 (EvaluationAgent) evaluates answer with clamped scores', async () => {
    const res = await nasikoOrchestrator.execute<any, any>('evaluation_agent', {
      question: {
        question_id: 'q_1',
        question: 'Explain the difference between clustered and non-clustered indexes in SQL.',
        topic: 'PostgreSQL',
        category: 'Databases',
        difficulty: 'medium',
        expected_concepts: ['Physical ordering', 'Table heap', 'B-Tree leaf pointers'],
      },
      candidate_answer: 'A clustered index determines the physical order of data in the table, so you can only have one per table. Non-clustered indexes have a separate structure with pointers back to the rows.',
      candidate_profile: {},
    });
    assert(Boolean(res.data.scores), 'Expected evaluation scores');
    assert(res.data.scores.overall >= 0.0 && res.data.scores.overall <= 10.0, 'Expected score in [0.0, 10.0]');
    assert(Boolean(res.data.feedback), 'Expected written feedback');
  });

  await runTest('4. Orchestration Pipeline', 'Agent 4 (InterviewReportAgent) generates comprehensive summary', async () => {
    const res = await nasikoOrchestrator.execute<any, any>('report_agent', {
      candidate_profile: { candidate: { name: 'Alice' } },
      questions: [
        { id: '1', question_id: 'q1', question: 'What is ACID?', topic: 'Databases', category: 'Backend', difficulty: 'medium' },
      ],
      answers: [
        { question_id: 'q1', answer: 'Atomicity, Consistency, Isolation, Durability guarantees transactions.' },
      ],
      evaluations: [
        {
          question_id: 'q1',
          scores: { technical_correctness: 9, completeness: 8, depth: 8, problem_solving: 8, communication: 9, practical_understanding: 8, overall: 8.3 },
          correct_concepts: ['Atomicity', 'Isolation'],
          incorrect_concepts: [],
          missing_concepts: [],
          misconceptions: [],
          strengths: ['Precise acronym definition'],
          weaknesses: [],
          feedback: 'Clear understanding of transaction safety.',
          recommended_followup_topics: ['Isolation levels'],
        },
      ],
    });
    assert(typeof res.data.overall_score === 'number', 'Expected numeric overall score');
    assert(res.data.overall_score >= 0.0 && res.data.overall_score <= 10.0, 'Overall score must be within [0, 10]');
    assert(Boolean(res.data.interview_summary), 'Expected interview summary');
  });

  await runTest('4. Orchestration Pipeline', 'Agent 5 (PreparationPlanAgent) generates structured study plan', async () => {
    const res = await nasikoOrchestrator.execute<any, any>('preparation_agent', {
      candidate_profile: { candidate: { name: 'Alice' } },
      interview_report: {
        overall_score: 8.0,
        technical_score: 8.5,
        communication_score: 8.0,
        problem_solving_score: 7.5,
        category_scores: { Backend: 8.0 },
        strengths: ['Clear terminology'],
        weaknesses: ['Deep dive into distributed systems'],
        topics_to_improve: ['Distributed locking', 'Event sourcing'],
        repeated_patterns_detected: [],
        interview_summary: 'Solid candidate overall.',
        detailed_feedback: 'Strong basics, recommend studying distributed consensus.',
        recommended_focus_areas: ['Raft protocol', 'Redis Redlock'],
      },
      evaluations: [],
    });
    assert(Array.isArray(res.data.study_plan), 'Expected study plan array');
    assert(res.data.study_plan.length > 0, 'Expected non-empty study plan');
    assert(res.data.study_plan[0].day > 0, 'Day must be positive integer');
  });

  // -------------------------------------------------------------
  // SUITE 5: INPUT CONTRACTS & ADVERSARIAL PAYLOAD RESILIENCE
  // -------------------------------------------------------------
  await runTest('5. Input Contracts', 'Interview input validator rejects missing profile/role', () => {
    assert(validateInterviewAgentInput({}).valid === false, 'Empty input must fail');
    assert(
      validateInterviewAgentInput({ candidate_profile: {}, interview_config: {} }).valid === false,
      'Missing role must fail'
    );
    const valid = validateInterviewAgentInput({
      candidate_profile: { candidate: { name: 'A' } },
      interview_config: { role: 'Backend', difficulty: 'medium', duration_minutes: 20, number_of_questions: 5 },
    });
    assert(valid.valid === true, 'Complete interview input must pass');
  });

  await runTest('5. Input Contracts', 'Evaluation input validator requires question + answer', () => {
    assert(validateEvaluationAgentInput({}).valid === false, 'Empty input must fail');
    assert(
      validateEvaluationAgentInput({ question: { question: 'Q?' }, candidate_answer: 123 as any }).valid ===
        false,
      'Non-string answer must fail'
    );
    const valid = validateEvaluationAgentInput({
      question: { question_id: 'q1', question: 'What is ACID?', topic: 'DB', category: 'Backend', difficulty: 'medium' },
      candidate_answer: 'Atomicity, Consistency, Isolation, Durability.',
    });
    assert(valid.valid === true, 'Valid evaluation input must pass');
  });

  await runTest('5. Input Contracts', 'Report input validator requires at least one evaluation', () => {
    assert(validateReportAgentInput({ evaluations: [] }).valid === false, 'Empty evaluations must fail');
    assert(validateReportAgentInput({}).valid === false, 'Missing evaluations must fail');
    assert(
      validateReportAgentInput({ evaluations: [{ question_id: 'q1' }] }).valid === true,
      'Non-empty evaluations must pass'
    );
  });

  await runTest('5. Input Contracts', 'Preparation input validator requires interview_report', () => {
    assert(validatePreparationPlanInput({}).valid === false, 'Missing report must fail');
    assert(
      validatePreparationPlanInput({ interview_report: { overall_score: 7 } }).valid === true,
      'Present report must pass'
    );
  });

  await runTest('5. Input Contracts', 'Validators survive adversarial payloads without throwing', () => {
    const xss = '<script>alert(1)</script>'.repeat(200);
    const sql = "'; DROP TABLE candidates; --".repeat(50);
    const long = 'a'.repeat(210000);
    // Must not throw; may validly accept long text (server enforces 200k/15k limits separately)
    for (const payload of [xss, sql, long]) {
      const r1 = validateResumeAgentInput({ resume_text: payload });
      assert(typeof r1.valid === 'boolean', 'Resume validator must return boolean for adversarial input');
      const r2 = validateEvaluationAgentInput({
        question: { question: 'Q?', topic: 'T', category: 'C', difficulty: 'medium' },
        candidate_answer: payload.slice(0, 20000),
      });
      assert(typeof r2.valid === 'boolean', 'Evaluation validator must return boolean for adversarial input');
    }
    // Server-side guardrail thresholds (mirrors server.ts: resume 200k, answer 15k)
    assert(long.length > 200000, 'Test payload exceeds resume limit');
    assert('x'.repeat(15001).length > 15000, 'Test payload exceeds answer limit');
  });

  // -------------------------------------------------------------
  // SUITE 6: LLM FALLBACK ENGINE & PROVIDER STATUS
  // -------------------------------------------------------------
  await runTest('6. LLM Resilience', 'Heuristic fallback synthesizes resume profile', () => {
    const data = generateHeuristicFallback<any>(
      'You are the Resume Reading Agent. Extract structured profile.',
      'RESUME CONTENT\nAlice Smith\nalice@example.com\n5 years Node.js, PostgreSQL, AWS.'
    );
    const check = validateResumeAgentOutput(data);
    assert(check.valid === true, `Fallback resume output must validate: ${check.error}`);
    assert(typeof data.candidate.name === 'string' && data.candidate.name.length > 0, 'Name synthesized');
  });

  await runTest('6. LLM Resilience', 'Heuristic fallback generates interview question', () => {
    const data = generateHeuristicFallback<any>(
      'You are the Interview Agent. Generate Question.',
      'Generate Question #2 of 5 for Senior Backend Engineer. RESUME: Node.js, Postgres.'
    );
    const check = validateInterviewAgentOutput(data);
    assert(check.valid === true, `Fallback question must validate: ${check.error}`);
    assert(String(data.question_id).startsWith('Q'), 'Question ID prefixed with Q');
  });

  await runTest('6. LLM Resilience', 'Heuristic fallback evaluation scores stay in [0,10]', () => {
    const data = generateHeuristicFallback<any>(
      'You are the Evaluation Agent. Evaluate the following candidate answer.',
      'Evaluate the following candidate answer.\nQuestion: What is ACID?\nCandidate\'s Answer: Atomicity, Consistency, Isolation, Durability.'
    );
    const check = validateEvaluationAgentOutput({ ...data, feedback: data.feedback || 'ok' });
    assert(check.valid === true, `Fallback evaluation must validate: ${check.error}`);
    assert(data.scores.overall >= 0 && data.scores.overall <= 10, 'Overall in bounds');
  });

  await runTest('6. LLM Resilience', 'Provider status reports a known provider + region', () => {
    const status = llmClient.getProviderStatus();
    assert(
      ['bedrock', 'gemini', 'openai', 'fallback'].includes(status.activeProvider),
      'activeProvider must be known'
    );
    assert(typeof status.activeModel === 'string' && status.activeModel.length > 0, 'activeModel set');
    assert(typeof status.bedrock.region === 'string', 'Bedrock region defaults (us-east-1)');
  });

  // -------------------------------------------------------------
  // SUITE 7: SCRAPER, OBSERVABILITY & NASIKO BRIDGE
  // -------------------------------------------------------------
  await runTest('7. Infra Clients', 'Scraper status exposes fallback-ready mode without key', () => {
    const status = anakinScraperClient.getStatus();
    assert(['anakin_api', 'fallback_ready'].includes(status.mode), 'Mode must be known');
    assert(Array.isArray(status.features) && status.features.length > 0, 'Features listed');
    assert(typeof status.endpoint === 'string' && status.endpoint.startsWith('http'), 'Endpoint set');
  });

  await runTest('7. Infra Clients', 'Scraper rejects malformed URL without network call', async () => {
    let threw = false;
    try {
      await anakinScraperClient.scrapeUrl('not-a-valid-url');
    } catch (err: any) {
      threw = true;
      assert(String(err.message).toLowerCase().includes('invalid url'), 'Error mentions invalid URL');
    }
    assert(threw === true, 'Malformed URL must throw');
  });

  await runTest('7. Infra Clients', 'Observability dashboard never crashes and normalizes traces', () => {
    const stats = nasikoObservability.getDashboardStats();
    assert(Array.isArray(stats.agents) && stats.agents.length === 5, 'All 5 agents registered');
    assert(typeof stats.summary.totalCalls === 'number', 'totalCalls numeric');
    assert(typeof stats.summary.avgLatencyMs === 'number', 'avgLatencyMs numeric');
    assert(Array.isArray(stats.recentTraces), 'recentTraces array');
    for (const t of (stats.recentTraces as any[]).slice(0, 3)) {
      assert(Boolean(t.timestamp || t.startedAt || t.completedAt), 'Trace has a timestamp field');
      assert(t.input !== undefined || t.inputSummary !== undefined, 'Trace has input shape');
    }
  });

  await runTest('7. Infra Clients', 'Nasiko bridge status exposes docker/embedded mode', () => {
    const status = nasikoClientBridge.getStatus();
    assert(['docker_nasiko', 'embedded_fallback'].includes(status.mode), 'Mode must be known');
    assert(typeof status.targetUrl === 'string' && status.targetUrl.length > 0, 'targetUrl set');
    assert(Array.isArray(status.registeredAgents) && status.registeredAgents.length === 5, '5 agents listed');
  });

  // -------------------------------------------------------------
  // SUITE 8: DATABASE EDGE CASES (REPORT/PLAN REPLACEMENT, LOOKUPS)
  // -------------------------------------------------------------
  await runTest('8. DB Edge Cases', 'Report creation replaces existing report for same session', () => {
    const base: any = {
      session_id: 'sess_replace_1',
      overall_score: 7,
      technical_score: 7,
      communication_score: 7,
      problem_solving_score: 7,
      category_scores: { Backend: 7 },
      strengths: ['A'],
      weaknesses: ['B'],
      topics_to_improve: ['C'],
      interview_summary: 'First',
      detailed_feedback: 'First detail',
      recommended_focus_areas: ['D'],
    };
    const r1 = testDb.createReport(base);
    const r2 = testDb.createReport({ ...base, overall_score: 8.5, interview_summary: 'Second' });
    const fetched = testDb.getReportBySessionId('sess_replace_1');
    assert(fetched?.overall_score === 8.5, 'Latest report wins');
    assert(fetched?.id === r2.id && r1.id !== r2.id, 'New report ID issued');
  });

  await runTest('8. DB Edge Cases', 'Plan creation replaces existing plan for same session', () => {
    const base: any = {
      session_id: 'sess_replace_1',
      learning_priorities: ['P1'],
      study_plan: [{ day: 1, topic: 'T', goals: ['g'], concepts: ['c'], practice_tasks: ['p'], estimated_minutes: 60 }],
      practice_questions: ['Q?'],
      coding_exercises: [],
      revision_topics: ['R'],
      recommended_next_interview: { suggested_role: 'Backend', focus_topics: [], target_difficulty: 'medium' },
    };
    testDb.createPlan(base);
    testDb.createPlan({ ...base, learning_priorities: ['P2'] });
    const fetched = testDb.getPlanBySessionId('sess_replace_1');
    assert(fetched?.learning_priorities[0] === 'P2', 'Latest plan wins');
  });

  await runTest('8. DB Edge Cases', 'Question lookup works by id and question_id; unknown lookups are undefined', () => {
    const qq = testDb.createQuestion({
      session_id: 'sess_lookup_1',
      question_id: 'Q099',
      question: 'What is a closure?',
      topic: 'JS',
      category: 'Frontend',
      difficulty: 'easy',
      reason: 'Lookup regression probe',
      expected_concepts: ['scope'],
      question_order: 1,
    });
    assert(testDb.getQuestion(qq.id)?.question_id === 'Q099', 'Lookup by internal id works');
    assert(testDb.getQuestion('Q099')?.id === qq.id, 'Lookup by question_id works');
    assert(testDb.getEvaluation('nope', 'nope') === undefined, 'Unknown evaluation is undefined');
    assert(testDb.getAnswer('nope', 'nope') === undefined, 'Unknown answer is undefined');
    assert(testDb.getSession('nope') === undefined, 'Unknown session is undefined');
  });

  // -------------------------------------------------------------
  // OUTPUT SUMMARY REPORT
  // -------------------------------------------------------------
  const total = results.length;
  const passed = results.filter((r) => r.passed).length;
  const failed = total - passed;

  console.log('\n===========================================================');
  console.log('📊 TEST EXECUTION SUMMARY:');
  console.log(`Total Tests Run: ${total}`);
  console.log(`✅ Passed: ${passed}`);
  console.log(`❌ Failed: ${failed}`);
  console.log('===========================================================');

  for (const r of results) {
    const icon = r.passed ? '✅' : '❌';
    console.log(`${icon} [${r.suite}] ${r.name} (${r.durationMs}ms)`);
    if (!r.passed && r.error) {
      console.log(`   Error: ${r.error}`);
    }
  }

  return { total, passed, failed, results };
}

// Auto-run if executed directly via tsx
if (
  (process.argv[1] && process.argv[1].includes('run-all-tests')) ||
  import.meta.url.includes('run-all-tests')
) {
  runFullQualitySuite()
    .then(({ failed }) => {
      if (failed > 0) {
        process.exit(1);
      }
      process.exit(0);
    })
    .catch((err) => {
      console.error('Test suite runner crashed:', err);
      process.exit(1);
    });
}
