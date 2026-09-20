export interface CompanyContext {
  name: string;
  products: string[];
  target_customers: string[];
}

export interface ResearchResult {
  title: string;
  url: string;
  snippet: string;
  date: string | null;
  last_updated: string | null;
}

export interface CompetitorResearch {
  competitor: string;
  results: ResearchResult[];
}

export type ReasoningStatusType = "pending" | "completed" | "insufficient_evidence" | "error";

export interface DronaHQRun {
  status: ReasoningStatusType;
  thread_id?: string | null;
  run_id?: string | null;
  message?: string | null;
}

export interface SignalOverlap {
  products: string[];
  customer_segments: string[];
}

export interface DepartmentImpact {
  product?: string | null;
  sales?: string | null;
  marketing?: string | null;
  strategy?: string | null;
}

export interface CompetitiveSignal {
  competitor: string;
  category?: string | null;
  headline?: string | null;
  summary?: string | null;
  significance?: string | null;
  overlap?: SignalOverlap;
  impact?: DepartmentImpact;
  recommended_actions?: string[];
  confidence?: string | null;
  source_urls: string[];
}

export interface ScanResponse {
  company: CompanyContext;
  research: CompetitorResearch[];
  reasoning: DronaHQRun;
  signals: CompetitiveSignal[];
}

export interface SignalContextSummary {
  competitor: string;
  headline?: string | null;
  category?: string | null;
  significance?: string | null;
}

export interface ProductResponse {
  actions: string[];
  investigation_questions: string[];
  caveats: string[];
}

export interface SalesResponse {
  trigger?: string | null;
  talk_track?: string | null;
  questions: string[];
  caveats: string[];
}

export interface MarketingResponse {
  actions: string[];
  messaging_angles: string[];
  caveats: string[];
}

export interface StrategyResponse {
  questions: string[];
  actions: string[];
  caveats: string[];
}

export interface SimulationEvidence {
  source_urls: string[];
}

export interface ResponseSimulation {
  signal_context: SignalContextSummary;
  product: ProductResponse;
  sales: SalesResponse;
  marketing: MarketingResponse;
  strategy: StrategyResponse;
  evidence: SimulationEvidence;
  confidence?: string | null;
}

export interface RelevantSignalRef {
  competitor: string;
  headline?: string | null;
}

export interface AskWarroomEvidence {
  source_urls: string[];
}

export interface AskWarroomResponse {
  answer: string;
  key_points: string[];
  relevant_signals: RelevantSignalRef[];
  evidence: AskWarroomEvidence;
  confidence: string;
  limitations: string[];
}
