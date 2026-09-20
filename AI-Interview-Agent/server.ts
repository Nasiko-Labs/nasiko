import express, { Request, Response } from 'express';
import http from 'http';
import path from 'path';
import multer from 'multer';
import mammoth from 'mammoth';
import dotenv from 'dotenv';
import { createServer as createViteServer } from 'vite';

import { db } from './server/db/database';
import { nasikoOrchestrator } from './server/nasiko/orchestrator';
import { nasikoObservability } from './server/nasiko/observability';
import { nasikoClientBridge } from './server/nasiko/clientBridge';
import { llmClient } from './server/llm/client';
import { registerAllAgents } from './server/agents';
import { SAMPLE_RESUMES } from './server/data/samples';
import { CandidateProfile } from './server/agents/resume_agent/schema';
import { InterviewAgentOutput } from './server/agents/interview_agent/schema';
import { EvaluationAgentOutput } from './server/agents/evaluation_agent/schema';
import { ReportAgentOutput } from './server/agents/report_agent/schema';
import { PreparationPlanOutput } from './server/agents/preparation_agent/schema';

import { anakinScraperClient } from './server/scraper/anakinClient';

dotenv.config();

// Global process guards to prevent crashes from unhandled errors
process.on('uncaughtException', (err) => {
  console.error('[CRITICAL] Uncaught exception prevented server crash:', err);
});
process.on('unhandledRejection', (reason) => {
  console.error('[CRITICAL] Unhandled rejection prevented server crash:', reason);
});

// Register all 5 agents with Nasiko Orchestration layer
registerAllAgents();

const upload = multer({
  storage: multer.memoryStorage(),
  limits: { fileSize: 20 * 1024 * 1024 }, // 20MB limit
});

const MAX_RESUME_TEXT_CHARS = 200000;

async function extractPdfText(buffer: Buffer): Promise<{ text: string; pageCount?: number }> {
  const { PDFParse } = await import('pdf-parse');
  const parser = new PDFParse({ data: buffer });

  try {
    const result = await parser.getText();
    const pages = Array.isArray(result.pages) ? result.pages : [];
    const pageText = pages
      .map((page) => page.text?.trim())
      .filter(Boolean)
      .map((text, index) => `--- Page ${index + 1} ---\n${text}`)
      .join('\n\n');

    let extractedText = (pageText || result.text || '').trim();
    
    // Check if extraction yielded meaningful content
    if (extractedText.length < 50) {
      throw new Error('Extracted text too short - PDF may be scanned/image-based or corrupted');
    }

    return {
      text: extractedText,
      pageCount: result.total,
    };
  } catch (err: any) {
    // Check for common PDF parsing errors
    if (err.message?.includes('Invalid PDF') || err.message?.includes('Password')) {
      throw new Error('PDF is password-protected, corrupted, or not a valid PDF file');
    }
    throw err;
  } finally {
    try {
      await parser.destroy();
    } catch {
      // Ignore cleanup errors
    }
  }
}

async function startServer() {
  const app = express();
  const PORT = 3000;

  app.use(express.json({ limit: '10mb' }));
  app.use(express.urlencoded({ extended: true, limit: '10mb' }));

  // ==========================================
  // API ROUTES
  // ==========================================

  // Health check
  app.get('/api/health', (_req: Request, res: Response) => {
    res.json({
      status: 'ok',
      timestamp: new Date().toISOString(),
      service: 'AI Interview Coach Multi-Agent Backend',
      orchestrator: 'Nasiko',
    });
  });

  // Comprehensive System Status (LLM Engine, Nasiko Docker status, Database, Anakin.io Scraper)
  app.get('/api/system/status', async (_req: Request, res: Response) => {
    const llm = llmClient.getProviderStatus();
    const nasiko = nasikoClientBridge.getStatus();
    const scraper = anakinScraperClient.getStatus();
    res.json({
      llm,
      nasiko,
      scraper,
      database: {
        type: 'embedded_json_sqlite',
        path: './data/interview_coach.json',
        configuredExternalUrl: process.env.DATABASE_URL ? 'sqlite' : 'none',
        requiresExternalUrl: false,
        status: 'healthy',
        message: 'Embedded storage active. No external database or DATABASE_URL needed.',
      },
    });
  });

  // ==========================================
  // ANAKIN.IO WEB SCRAPING API ROUTES
  // ==========================================

  // Get Anakin.io Scraper Status
  app.get('/api/scraper/status', (_req: Request, res: Response) => {
    res.json(anakinScraperClient.getStatus());
  });

  // Scrape any URL via Anakin.io (or resilient fallback)
  app.post('/api/scraper/url', async (req: Request, res: Response): Promise<void> => {
    try {
      const { url, render_js = true, extract_markdown = true } = req.body;
      if (!url) {
        res.status(400).json({ error: 'Target "url" parameter is required' });
        return;
      }

      const result = await anakinScraperClient.scrapeUrl(url, {
        renderJs: render_js,
        extractMarkdown: extract_markdown,
      });

      res.json(result);
    } catch (err: any) {
      console.error('Error in /api/scraper/url:', err);
      res.status(500).json({ error: err.message || 'Web scraping failed' });
    }
  });

  // Scrape Target Job Description (e.g. from LinkedIn, Greenhouse, Lever, Ashby, Indeed)
  app.post('/api/scraper/job', async (req: Request, res: Response): Promise<void> => {
    try {
      const { url } = req.body;
      if (!url) {
        res.status(400).json({ error: 'Job posting "url" parameter is required' });
        return;
      }

      const result = await anakinScraperClient.scrapeJobPosting(url);
      res.json(result);
    } catch (err: any) {
      console.error('Error in /api/scraper/job:', err);
      res.status(500).json({ error: err.message || 'Failed to scrape job posting' });
    }
  });

  // Scrape Candidate Portfolio/GitHub/Resume Link and Analyze with Agent 1
  app.post('/api/scraper/scrape-resume', async (req: Request, res: Response): Promise<void> => {
    try {
      const { url } = req.body;
      if (!url) {
        res.status(400).json({ error: 'Candidate profile or portfolio "url" is required' });
        return;
      }

      const scrapeResult = await anakinScraperClient.scrapeUrl(url);
      const textToAnalyze = `${scrapeResult.title}\nSource URL: ${url}\nScraper Engine: ${scrapeResult.provider}\n\n${scrapeResult.textContent}`;

      if (textToAnalyze.trim().length < 40) {
        res.status(400).json({
          error: 'The scraped webpage did not contain enough text for resume analysis.',
        });
        return;
      }

      // Execute Resume Reading Agent (Agent 1)
      const result = await nasikoOrchestrator.execute<any, CandidateProfile>(
        'resume_agent',
        { resume_text: textToAnalyze, file_name: `web_${new URL(url).hostname}` },
        { tags: { source: 'anakin_scraper', type: 'web_ingestion' } }
      );

      // Persist in DB
      const candidate = db.createCandidate({
        name: result.data.candidate?.name || scrapeResult.title || 'Web Candidate',
        email: result.data.candidate?.email || '',
        phone: result.data.candidate?.phone || '',
        location: result.data.candidate?.location || '',
        resume_text: textToAnalyze,
        candidate_profile: result.data,
      });

      res.json({
        candidate_id: candidate.id,
        candidate_profile: result.data,
        scraped_meta: {
          title: scrapeResult.title,
          url: scrapeResult.url,
          provider: scrapeResult.provider,
          latency_ms: scrapeResult.latencyMs,
        },
        trace_id: result.traceId,
      });
    } catch (err: any) {
      console.error('Error in /api/scraper/scrape-resume:', err);
      res.status(500).json({ error: err.message || 'Failed to scrape and analyze profile' });
    }
  });

  // Re-probe local Docker Nasiko on demand
  app.post('/api/nasiko/probe', async (_req: Request, res: Response) => {
    const isReachable = await nasikoClientBridge.probeDockerInstance();
    const status = nasikoClientBridge.getStatus();
    res.json({
      reachable: isReachable,
      status,
    });
  });

  // Sample resumes for instant testing
  app.get('/api/resume/samples', (_req: Request, res: Response) => {
    res.json({ samples: SAMPLE_RESUMES });
  });

  // Upload and parse resume file (PDF, DOCX, TXT)
  app.post('/api/resume/upload', upload.single('resume'), async (req: Request, res: Response): Promise<void> => {
    try {
      if (!req.file && !req.body.resume_text) {
        res.status(400).json({ error: 'No file uploaded or resume_text provided' });
        return;
      }

      let extractedText = '';
      let fileName = 'pasted_resume.txt';

      if (req.file) {
        fileName = req.file.originalname;
        const mime = req.file.mimetype;
        const buffer = req.file.buffer;

if (mime === 'application/pdf' || fileName.toLowerCase().endsWith('.pdf')) {
          try {
            const parsed = await extractPdfText(buffer);
            extractedText = parsed.pageCount
              ? `[PDF pages extracted: ${parsed.pageCount}]\n\n${parsed.text}`
              : parsed.text;
            console.log(`[PDF Extract] ${fileName}: ${parsed.pageCount} pages, ${extractedText.length} chars extracted`);
          } catch (pdfErr) {
            console.error('PDF parsing error:', pdfErr);
            res.status(400).json({
              error:
                'Could not extract readable text from this PDF. If it is scanned or image-only, please upload a text-based PDF, DOCX, or pasted text.',
            });
            return;
          }
        } else if (
          mime === 'application/vnd.openxmlformats-officedocument.wordprocessingml.document' ||
          fileName.toLowerCase().endsWith('.docx')
        ) {
          const result = await mammoth.extractRawText({ buffer });
          extractedText = result.value;
        } else {
          // Plain text / markdown / fallback
          extractedText = buffer.toString('utf-8');
        }
      } else {
        extractedText = req.body.resume_text;
      }

      if (!extractedText || extractedText.trim().length < 20) {
        res.status(400).json({
          error:
            'Extracted text is too short or invalid. If this is a scanned PDF, convert it with OCR or upload a text-based resume.',
        });
        return;
      }
      if (extractedText.length > MAX_RESUME_TEXT_CHARS) {
        res.status(413).json({
          error: `Extracted resume text exceeds limit of ${MAX_RESUME_TEXT_CHARS.toLocaleString()} characters`,
        });
        return;
      }

      res.json({
        file_name: fileName,
        resume_text: extractedText.trim(),
        character_count: extractedText.length,
      });
    } catch (err: any) {
      console.error('Error in /api/resume/upload:', err);
      res.status(500).json({ error: `File extraction failed: ${err.message}` });
    }
  });

// Agent 1: Analyze Resume with Nasiko ResumeReadingAgent
  app.post('/api/resume/analyze', async (req: Request, res: Response): Promise<void> => {
    try {
      const { resume_text, file_name } = req.body;
      if (!resume_text || typeof resume_text !== 'string' || resume_text.trim().length < 20) {
        res.status(400).json({ error: 'Valid resume_text is required (minimum 20 characters)' });
        return;
      }
      if (resume_text.length > MAX_RESUME_TEXT_CHARS) {
        res.status(413).json({
          error: `Payload too large: resume_text exceeds limit of ${MAX_RESUME_TEXT_CHARS.toLocaleString()} characters`,
        });
        return;
      }

      console.log(`[Resume Analyze] ${file_name || 'unknown'}: ${resume_text.length} chars, provider: ${llmClient.getProviderStatus().activeProvider}`);

      // Execute via Nasiko
      const result = await nasikoOrchestrator.execute<
        { resume_text: string; file_name?: string },
        CandidateProfile
      >('resume_agent', { resume_text, file_name });

      console.log(`[Resume Analyze] Completed in ${result.latencyMs}ms via ${result.llmLatencyMs ? 'LLM' : 'fallback'}, tokens: ${JSON.stringify(result.tokens)}`);

      // Persist Candidate to database
      const candidate = db.createCandidate({
        name: result.data.candidate.name || 'Candidate',
        email: result.data.candidate.email || '',
        phone: result.data.candidate.phone,
        location: result.data.candidate.location,
        resume_text,
        candidate_profile: result.data,
      });

      res.json({
        candidate_id: candidate.id,
        candidate_profile: result.data,
        latency_ms: result.latencyMs,
        trace_id: result.traceId,
      });
    } catch (err: any) {
      console.error('Error in /api/resume/analyze:', err);
      res.status(500).json({ error: err.message || 'Failed to analyze resume' });
    }
  });

  // Start Interview Session & Generate Question 1
  app.post('/api/interview/start', async (req: Request, res: Response): Promise<void> => {
    try {
      const { candidate_id, candidate_profile, role, difficulty = 'medium', number_of_questions = 5, duration_minutes = 20 } = req.body;

      let profile = candidate_profile;
      let candId = candidate_id;

      if (!profile && candidate_id) {
        const candidate = db.getCandidate(candidate_id);
        if (candidate) {
          profile = candidate.candidate_profile;
        }
      }

      if (!profile) {
        res.status(400).json({ error: 'Candidate profile is required to start an interview' });
        return;
      }

      if (!candId) {
        const candidate = db.createCandidate({
          name: profile.candidate?.name || 'Candidate',
          email: profile.candidate?.email || '',
          resume_text: '',
          candidate_profile: profile,
        });
        candId = candidate.id;
      }

      // 1. Create Session in DB
      const session = db.createSession({
        candidate_id: candId,
        target_role: role || profile.seniority_estimate || 'Software Engineer',
        difficulty,
        status: 'in_progress',
        duration_minutes,
        number_of_questions,
        current_question_index: 1,
      });

      // 2. Call Interview Agent via Nasiko to generate Q1
      const result = await nasikoOrchestrator.execute<any, InterviewAgentOutput>(
        'interview_agent',
        {
          candidate_profile: profile,
          interview_config: {
            role: session.target_role,
            difficulty: session.difficulty,
            duration_minutes: session.duration_minutes,
            number_of_questions: session.number_of_questions,
          },
          conversation_history: [],
        },
        { sessionId: session.id, candidateId: candId }
      );

      // 3. Save Question in DB
      const question = db.createQuestion({
        session_id: session.id,
        question_id: result.data.question_id || 'Q001',
        question: result.data.question,
        topic: result.data.topic,
        category: result.data.category,
        difficulty: result.data.difficulty,
        reason: result.data.reason,
        expected_concepts: result.data.expected_concepts,
        question_order: 1,
      });

      res.json({
        session,
        question,
        interview_state: result.data.interview_state,
        trace_id: result.traceId,
      });
    } catch (err: any) {
      console.error('Error in /api/interview/start:', err);
      res.status(500).json({ error: err.message || 'Failed to start interview' });
    }
  });

  // Submit Answer -> Agent 3 (Evaluation) -> Agent 2 (Next Question or Complete)
  app.post('/api/interview/answer', async (req: Request, res: Response): Promise<void> => {
    try {
      const { session_id, question_id, answer } = req.body;
      if (!session_id || !question_id || answer === undefined) {
        res.status(400).json({ error: 'session_id, question_id, and answer are required' });
        return;
      }

      if (typeof answer !== 'string') {
        res.status(400).json({ error: 'answer must be a string' });
        return;
      }

      // Max payload length guardrail
      if (answer.length > 15000) {
        res.status(413).json({ error: 'Payload too large: answer exceeds maximum limit of 15,000 characters' });
        return;
      }

      const session = db.getSession(session_id);
      if (!session) {
        res.status(404).json({ error: 'Interview session not found' });
        return;
      }

      // State Machine Guardrail: Session must be in_progress
      if (session.status === 'completed') {
        res.status(409).json({ error: 'Interview session is already completed. Cannot submit additional answers.' });
        return;
      }

      const question = db.getQuestion(question_id);
      if (!question) {
        res.status(404).json({ error: 'Interview question not found' });
        return;
      }

      // State Machine Guardrail: Question must belong to the requested session
      if (question.session_id !== session.id) {
        res.status(400).json({ error: 'Question does not belong to the specified interview session' });
        return;
      }

      // Idempotency Guardrail: If answer & evaluation already recorded, return existing record
      const existingEvaluation = db.getEvaluation(session.id, question.id);
      if (existingEvaluation) {
        const allQuestions = db.getQuestionsForSession(session.id);
        const currentCount = allQuestions.length;
        const isCompleted = currentCount >= session.number_of_questions;
        const nextQ = isCompleted
          ? null
          : allQuestions.find((q) => q.question_order > question.question_order) || null;

        res.json({
          evaluation: existingEvaluation,
          is_completed: isCompleted,
          next_question: nextQ,
          idempotent: true,
          message: 'Evaluation retrieved from existing record (idempotent request)',
        });
        return;
      }

      const candidate = db.getCandidate(session.candidate_id);
      const profile = candidate?.candidate_profile || {};

      // 1. Save Answer
      const savedAnswer = db.createAnswer({
        question_id: question.id,
        session_id: session.id,
        answer: String(answer).trim(),
      });

      // 2. Call Agent 3 (Evaluation Agent) via Nasiko
      const evalResult = await nasikoOrchestrator.execute<any, EvaluationAgentOutput>(
        'evaluation_agent',
        {
          question: {
            question_id: question.question_id,
            question: question.question,
            topic: question.topic,
            category: question.category,
            difficulty: question.difficulty,
            expected_concepts: question.expected_concepts,
          },
          candidate_answer: savedAnswer.answer,
          candidate_profile: profile,
        },
        { sessionId: session.id, candidateId: session.candidate_id }
      );

      // 3. Save Evaluation
      const savedEvaluation = db.createEvaluation({
        answer_id: savedAnswer.id,
        question_id: question.id,
        session_id: session.id,
        scores: evalResult.data.scores,
        strengths: evalResult.data.strengths,
        weaknesses: evalResult.data.weaknesses,
        missing_concepts: evalResult.data.missing_concepts,
        incorrect_concepts: evalResult.data.incorrect_concepts,
        feedback: evalResult.data.feedback,
        recommended_followup_topics: evalResult.data.recommended_followup_topics,
      });

      // 4. Check if interview is completed
      const allQuestions = db.getQuestionsForSession(session.id);
      const currentCount = allQuestions.length;

      if (currentCount >= session.number_of_questions) {
        // Complete interview
        db.updateSession(session.id, {
          status: 'completed',
          completed_at: new Date().toISOString(),
        });

        res.json({
          evaluation: savedEvaluation,
          is_completed: true,
          next_question: null,
          message: 'All questions completed! Proceed to generate interview report.',
        });
        return;
      }

      // 5. Generate Next Question using Agent 2 (Interview Agent) with updated adaptive conversation history
      const allAnswers = db.getAnswersForSession(session.id);
      const allEvals = db.getEvaluationsForSession(session.id);

      const conversationHistory = allQuestions.map((q) => {
        const a = allAnswers.find((ans) => ans.question_id === q.id);
        const ev = allEvals.find((e) => e.question_id === q.id);
        return {
          question_id: q.question_id,
          question: q.question,
          topic: q.topic,
          category: q.category,
          difficulty: q.difficulty,
          answer: a?.answer,
          evaluation: ev
            ? {
                scores: ev.scores,
                strengths: ev.strengths,
                weaknesses: ev.weaknesses,
                missing_concepts: ev.missing_concepts,
              }
            : undefined,
        };
      });

      const nextQuestionResult = await nasikoOrchestrator.execute<any, InterviewAgentOutput>(
        'interview_agent',
        {
          candidate_profile: profile,
          interview_config: {
            role: session.target_role,
            difficulty: session.difficulty,
            duration_minutes: session.duration_minutes,
            number_of_questions: session.number_of_questions,
          },
          conversation_history: conversationHistory,
        },
        { sessionId: session.id, candidateId: session.candidate_id }
      );

      const nextQ = db.createQuestion({
        session_id: session.id,
        question_id: nextQuestionResult.data.question_id || `Q${String(currentCount + 1).padStart(3, '0')}`,
        question: nextQuestionResult.data.question,
        topic: nextQuestionResult.data.topic,
        category: nextQuestionResult.data.category,
        difficulty: nextQuestionResult.data.difficulty,
        reason: nextQuestionResult.data.reason,
        expected_concepts: nextQuestionResult.data.expected_concepts,
        question_order: currentCount + 1,
      });

      db.updateSession(session.id, { current_question_index: currentCount + 1 });

      res.json({
        evaluation: savedEvaluation,
        is_completed: false,
        next_question: nextQ,
        interview_state: nextQuestionResult.data.interview_state,
      });
    } catch (err: any) {
      console.error('Error in /api/interview/answer:', err);
      res.status(500).json({ error: err.message || 'Failed to submit answer' });
    }
  });

  // Get Session Details
  app.get('/api/interview/:id', (req: Request, res: Response): void => {
    const sessionId = req.params.id;
    const session = db.getSession(sessionId);
    if (!session) {
      res.status(404).json({ error: 'Session not found' });
      return;
    }

    const candidate = db.getCandidate(session.candidate_id);
    const questions = db.getQuestionsForSession(sessionId);
    const answers = db.getAnswersForSession(sessionId);
    const evaluations = db.getEvaluationsForSession(sessionId);
    const report = db.getReportBySessionId(sessionId);
    const plan = db.getPlanBySessionId(sessionId);

    res.json({
      session,
      candidate,
      questions,
      answers,
      evaluations,
      report,
      plan,
    });
  });

  // Agent 4: Generate or Get Interview Report
  app.post('/api/report/generate', async (req: Request, res: Response): Promise<void> => {
    try {
      const { session_id } = req.body;
      if (!session_id) {
        res.status(400).json({ error: 'session_id is required' });
        return;
      }

      const session = db.getSession(session_id);
      if (!session) {
        res.status(404).json({ error: 'Session not found' });
        return;
      }

      const existingReport = db.getReportBySessionId(session_id);
      if (existingReport && !req.body.force_regenerate) {
        res.json({ report: existingReport, from_cache: true });
        return;
      }

      const candidate = db.getCandidate(session.candidate_id);
      const questions = db.getQuestionsForSession(session_id);
      const answers = db.getAnswersForSession(session_id);
      const evaluations = db.getEvaluationsForSession(session_id);

      if (evaluations.length === 0) {
        res.status(400).json({ error: 'Cannot generate report: No evaluations recorded yet' });
        return;
      }

      // Execute Agent 4 via Nasiko
      const reportResult = await nasikoOrchestrator.execute<any, ReportAgentOutput>(
        'report_agent',
        {
          candidate_profile: candidate?.candidate_profile || {},
          questions: questions.map((q) => ({
            id: q.id,
            question_id: q.question_id,
            question: q.question,
            topic: q.topic,
            category: q.category,
            difficulty: q.difficulty,
          })),
          answers: answers.map((a) => ({
            question_id: a.question_id,
            answer: a.answer,
          })),
          evaluations: evaluations.map((e) => ({
            question_id: e.question_id,
            scores: e.scores,
            correct_concepts: [],
            incorrect_concepts: e.incorrect_concepts || [],
            missing_concepts: e.missing_concepts || [],
            misconceptions: [],
            strengths: e.strengths || [],
            weaknesses: e.weaknesses || [],
            feedback: e.feedback || '',
            recommended_followup_topics: e.recommended_followup_topics || [],
          })),
        },
        { sessionId: session_id, candidateId: session.candidate_id }
      );

      const savedReport = db.createReport({
        session_id,
        overall_score: reportResult.data.overall_score,
        technical_score: reportResult.data.technical_score,
        communication_score: reportResult.data.communication_score,
        problem_solving_score: reportResult.data.problem_solving_score,
        category_scores: reportResult.data.category_scores,
        strengths: reportResult.data.strengths,
        weaknesses: reportResult.data.weaknesses,
        topics_to_improve: reportResult.data.topics_to_improve,
        interview_summary: reportResult.data.interview_summary,
        detailed_feedback: reportResult.data.detailed_feedback,
        recommended_focus_areas: reportResult.data.recommended_focus_areas,
      });

      res.json({
        report: savedReport,
        trace_id: reportResult.traceId,
        latency_ms: reportResult.latencyMs,
      });
    } catch (err: any) {
      console.error('Error in /api/report/generate:', err);
      res.status(500).json({ error: err.message || 'Failed to generate report' });
    }
  });

  app.get('/api/report/:interviewId', (req: Request, res: Response): void => {
    const report = db.getReportBySessionId(req.params.interviewId);
    if (!report) {
      res.status(404).json({ error: 'Report not found for session' });
      return;
    }
    res.json({ report });
  });

  // Agent 5: Generate or Get Personalized Preparation Plan
  app.post('/api/preparation/generate', async (req: Request, res: Response): Promise<void> => {
    try {
      const { session_id } = req.body;
      if (!session_id) {
        res.status(400).json({ error: 'session_id is required' });
        return;
      }

      const session = db.getSession(session_id);
      if (!session) {
        res.status(404).json({ error: 'Session not found' });
        return;
      }

      const existingPlan = db.getPlanBySessionId(session_id);
      if (existingPlan && !req.body.force_regenerate) {
        res.json({ plan: existingPlan, from_cache: true });
        return;
      }

      let report = db.getReportBySessionId(session_id);
      const evaluations = db.getEvaluationsForSession(session_id);
      const candidate = db.getCandidate(session.candidate_id);

      if (!report) {
        if (evaluations.length === 0) {
          res.status(400).json({ error: 'Cannot generate preparation plan: No evaluations completed for this session yet' });
          return;
        }

        // Automatically generate report if not present yet
        const questions = db.getQuestionsForSession(session_id);
        const answers = db.getAnswersForSession(session_id);
        const reportResult = await nasikoOrchestrator.execute<any, ReportAgentOutput>(
          'report_agent',
          {
            candidate_profile: candidate?.candidate_profile || {},
            questions,
            answers,
            evaluations: evaluations.map((e) => ({
              question_id: e.question_id,
              scores: e.scores,
              correct_concepts: [],
              incorrect_concepts: e.incorrect_concepts || [],
              missing_concepts: e.missing_concepts || [],
              misconceptions: [],
              strengths: e.strengths || [],
              weaknesses: e.weaknesses || [],
              feedback: e.feedback || '',
              recommended_followup_topics: e.recommended_followup_topics || [],
            })),
          },
          { sessionId: session_id }
        );
        report = db.createReport({
          session_id,
          overall_score: reportResult.data.overall_score,
          technical_score: reportResult.data.technical_score,
          communication_score: reportResult.data.communication_score,
          problem_solving_score: reportResult.data.problem_solving_score,
          category_scores: reportResult.data.category_scores,
          strengths: reportResult.data.strengths,
          weaknesses: reportResult.data.weaknesses,
          topics_to_improve: reportResult.data.topics_to_improve,
          interview_summary: reportResult.data.interview_summary,
          detailed_feedback: reportResult.data.detailed_feedback,
          recommended_focus_areas: reportResult.data.recommended_focus_areas,
        });
      }

      // Execute Agent 5 via Nasiko
      const planResult = await nasikoOrchestrator.execute<any, PreparationPlanOutput>(
        'preparation_agent',
        {
          candidate_profile: candidate?.candidate_profile || {},
          interview_report: report,
          evaluations: evaluations.map((e) => ({
            question_id: e.question_id,
            scores: e.scores,
            correct_concepts: [],
            incorrect_concepts: e.incorrect_concepts || [],
            missing_concepts: e.missing_concepts || [],
            misconceptions: [],
            strengths: e.strengths || [],
            weaknesses: e.weaknesses || [],
            feedback: e.feedback || '',
            recommended_followup_topics: e.recommended_followup_topics || [],
          })),
        },
        { sessionId: session_id, candidateId: session.candidate_id }
      );

      const savedPlan = db.createPlan({
        session_id,
        learning_priorities: planResult.data.learning_priorities,
        study_plan: planResult.data.study_plan,
        practice_questions: planResult.data.practice_questions,
        coding_exercises: planResult.data.coding_exercises,
        revision_topics: planResult.data.revision_topics,
        recommended_next_interview: planResult.data.recommended_next_interview,
      });

      res.json({
        plan: savedPlan,
        trace_id: planResult.traceId,
        latency_ms: planResult.latencyMs,
      });
    } catch (err: any) {
      console.error('Error in /api/preparation/generate:', err);
      res.status(500).json({ error: err.message || 'Failed to generate preparation plan' });
    }
  });

  app.get('/api/preparation/:interviewId', (req: Request, res: Response): void => {
    const plan = db.getPlanBySessionId(req.params.interviewId);
    if (!plan) {
      res.status(404).json({ error: 'Preparation plan not found for session' });
      return;
    }
    res.json({ plan });
  });

  // Nasiko Observability API (Developer / Debug View)
  app.get('/api/observability/stats', (_req: Request, res: Response) => {
    const stats = nasikoObservability.getDashboardStats();
    res.json(stats);
  });

  // Seed sample demo session if database is empty for instantaneous demo exploration
  app.post('/api/demo/seed', async (_req: Request, res: Response) => {
    try {
      const javaSample = SAMPLE_RESUMES[0];
      // Check if candidate already exists
      let candidate = db.getAllCandidates().find((c) => c.name === 'Alex Rivera');
      if (!candidate) {
        candidate = db.createCandidate({
          name: 'Alex Rivera',
          email: 'alex.rivera.dev@gmail.com',
          phone: '+1 (555) 432-8765',
          location: 'Austin, TX',
          resume_text: javaSample.resumeText,
          candidate_profile: {
            candidate: {
              name: 'Alex Rivera',
              email: 'alex.rivera.dev@gmail.com',
              phone: '+1 (555) 432-8765',
              location: 'Austin, TX',
            },
            education: [
              {
                degree: 'Bachelor of Science in Computer Science',
                university: 'University of Texas at Austin',
                graduation_year: '2023',
                cgpa: '3.82 / 4.0',
              },
            ],
            experience: [
              {
                company: 'Apex FinTech Solutions',
                role: 'Software Engineer',
                duration: 'July 2023 - Present',
                responsibilities: [
                  'Designed and maintained scalable Spring Boot microservices handling 5,000 req/s',
                  'Optimized MySQL queries reducing p99 latency by 38%',
                ],
                technologies: ['Java 17', 'Spring Boot 3', 'MySQL', 'Docker', 'AWS'],
                domain: 'Fintech',
              },
            ],
            skills: {
              languages: ['Java (8/11/17)', 'SQL', 'Bash'],
              backend: ['Spring Boot', 'Spring MVC', 'Spring Data JPA', 'Hibernate', 'Spring Security'],
              frontend: [],
              databases: ['MySQL', 'PostgreSQL', 'Redis'],
              cloud: ['AWS (EC2, S3, RDS)'],
              devops: ['Docker', 'Kubernetes', 'Jenkins CI/CD'],
              frameworks: ['Hibernate', 'Flyway'],
              testing: ['JUnit 5', 'Mockito', 'Testcontainers'],
              other: ['Microservices', 'RESTful APIs', 'JWT', 'ACID Transactions'],
            },
            projects: [
              {
                name: 'Employee Management System & Payroll Engine',
                description: 'Multi-tenant employee management platform with payroll calculation and JWT auth.',
                technologies: ['Java 17', 'Spring Boot', 'Spring Security', 'MySQL', 'Docker'],
                candidate_contribution: 'Architected security layer, background auditing, and database schema',
                technical_concepts: ['JWT Authentication', 'Spring Security Filter Chain', 'Spring @Scheduled'],
                potential_interview_topics: ['Token validation lifecycle', 'Database concurrency'],
              },
            ],
            certifications: ['AWS Certified Solutions Architect', 'Oracle Certified Professional Java SE 11'],
            interview_topics: ['Spring Boot Microservices', 'Spring Security & JWT', 'MySQL Optimization'],
            seniority_estimate: 'Mid-level (3-4 years)',
            resume_summary: 'Solid Java/Spring Boot engineer with direct fintech production experience.',
          },
        });
      }

      res.json({ status: 'seeded', candidate });
    } catch (err: any) {
      res.status(500).json({ error: err.message });
    }
  });

  // Automated Code Quality & Testing Suite Endpoint
  app.post('/api/testing/run-suite', async (_req: Request, res: Response): Promise<void> => {
    try {
      const { runFullQualitySuite } = await import('./tests/run-all-tests');
      const testReport = await runFullQualitySuite();
      res.json({
        success: testReport.failed === 0,
        ...testReport,
        timestamp: new Date().toISOString(),
      });
    } catch (err: any) {
      console.error('Error running test suite via API:', err);
      res.status(500).json({ error: 'Failed to execute test suite', details: err?.message });
    }
  });

  // ==========================================
  // VITE DEV OR PRODUCTION STATIC SERVING
  // ==========================================
  // NOTE: Vite runs in middlewareMode, so its HMR WebSocket server must be
  // explicitly attached to the same underlying HTTP server. Previously
  // `app.listen()` was used with `hmr: undefined`, leaving Vite's WS server
  // orphaned and causing `WebSocket connection to 'ws://.../ ' failed` in the
  // browser. We now create the HTTP server first and hand it to Vite.
  const httpServer = http.createServer(app);

  if (process.env.NODE_ENV !== 'production') {
    const hmrDisabled = process.env.DISABLE_HMR === 'true';
    const vite = await createViteServer({
      server: {
        middlewareMode: true,
        hmr: hmrDisabled ? false : { server: httpServer },
      },
      appType: 'spa',
    });
    app.use(vite.middlewares);
  } else {
    const distPath = path.join(process.cwd(), 'dist');
    app.use(express.static(distPath));
    // `/*` (instead of `*`) keeps the SPA fallback working on both
    // Express 4 and Express 5 (where `*` throws a PathError).
    app.get('/*', (_req: Request, res: Response) => {
      res.sendFile(path.join(distPath, 'index.html'));
    });
  }

  // Prevent stray WebSocket upgrade errors from crashing the process.
  httpServer.on('upgrade', (_req, socket) => {
    // Vite HMR handles its own upgrades; destroy anything unhandled
    // after a tick so a rogue client can't hang the socket forever.
    const s = socket as any;
    if (typeof s.setTimeout === 'function') {
      s.setTimeout(5000, () => {
        try {
          s.destroy();
        } catch {
          // ignore
        }
      });
    }
  });

  httpServer.listen(PORT, '0.0.0.0', () => {
    console.log(`[AI Interview Coach] Server running at http://0.0.0.0:${PORT}`);
    console.log(`[Nasiko] Agent Orchestration layer active on port ${PORT}`);
  });
}

startServer().catch((err) => {
  console.error('Fatal server startup error:', err);
});
