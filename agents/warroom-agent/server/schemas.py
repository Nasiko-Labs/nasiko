from typing import Literal
from pydantic import BaseModel, Field, field_validator

ReasoningStatus = Literal["pending", "completed", "insufficient_evidence", "error"]


class CompanyContext(BaseModel):
    name: str
    products: list[str] = Field(default_factory=list)
    target_customers: list[str] = Field(default_factory=list)


class CompetitorConfig(BaseModel):
    name: str


class ResearchResult(BaseModel):
    title: str
    url: str
    snippet: str
    date: str | None = None
    last_updated: str | None = None


class CompetitorResearch(BaseModel):
    competitor: str
    results: list[ResearchResult] = Field(default_factory=list)


class DronaHQRun(BaseModel):
    status: ReasoningStatus
    thread_id: str | None = None
    run_id: str | None = None
    message: str | None = None


class SignalOverlap(BaseModel):
    products: list[str] = Field(default_factory=list)
    customer_segments: list[str] = Field(default_factory=list)


class DepartmentImpact(BaseModel):
    product: str | None = None
    sales: str | None = None
    marketing: str | None = None
    strategy: str | None = None


class CompetitiveSignal(BaseModel):
    competitor: str
    category: str | None = None
    headline: str | None = None
    summary: str | None = None
    significance: str | None = None
    overlap: SignalOverlap = Field(default_factory=SignalOverlap)
    impact: DepartmentImpact = Field(default_factory=DepartmentImpact)
    recommended_actions: list[str] = Field(default_factory=list)
    confidence: str | None = None
    source_urls: list[str] = Field(default_factory=list)


class ScanRequest(BaseModel):
    company: CompanyContext | None = None
    competitors: list[CompetitorConfig] | None = None


class ScanResponse(BaseModel):
    company: CompanyContext
    research: list[CompetitorResearch] = Field(default_factory=list)
    reasoning: DronaHQRun
    signals: list[CompetitiveSignal] = Field(default_factory=list)


class SignalContextSummary(BaseModel):
    competitor: str
    headline: str | None = None
    category: str | None = None
    significance: str | None = None


class ProductResponse(BaseModel):
    actions: list[str] = Field(default_factory=list)
    investigation_questions: list[str] = Field(default_factory=list)
    caveats: list[str] = Field(default_factory=list)


class SalesResponse(BaseModel):
    trigger: str | None = None
    talk_track: str | None = None
    questions: list[str] = Field(default_factory=list)
    caveats: list[str] = Field(default_factory=list)


class MarketingResponse(BaseModel):
    actions: list[str] = Field(default_factory=list)
    messaging_angles: list[str] = Field(default_factory=list)
    caveats: list[str] = Field(default_factory=list)


class StrategyResponse(BaseModel):
    questions: list[str] = Field(default_factory=list)
    actions: list[str] = Field(default_factory=list)
    caveats: list[str] = Field(default_factory=list)


class SimulationEvidence(BaseModel):
    source_urls: list[str] = Field(default_factory=list)


class ResponseSimulation(BaseModel):
    signal_context: SignalContextSummary
    product: ProductResponse = Field(default_factory=ProductResponse)
    sales: SalesResponse = Field(default_factory=SalesResponse)
    marketing: MarketingResponse = Field(default_factory=MarketingResponse)
    strategy: StrategyResponse = Field(default_factory=StrategyResponse)
    evidence: SimulationEvidence = Field(default_factory=SimulationEvidence)
    confidence: str | None = None


class SimulateRequest(BaseModel):
    signal: CompetitiveSignal
    company: CompanyContext | None = None


class RelevantSignalRef(BaseModel):
    competitor: str
    headline: str | None = None


class AskWarroomEvidence(BaseModel):
    source_urls: list[str] = Field(default_factory=list)


class AskWarroomResponse(BaseModel):
    answer: str
    key_points: list[str] = Field(default_factory=list)
    relevant_signals: list[RelevantSignalRef] = Field(default_factory=list)
    evidence: AskWarroomEvidence = Field(default_factory=AskWarroomEvidence)
    confidence: str = "medium"
    limitations: list[str] = Field(default_factory=list)


class AskWarroomRequest(BaseModel):
    question: str = Field(..., min_length=1, max_length=1000)
    signals: list[CompetitiveSignal] = Field(default_factory=list)
    company: CompanyContext | None = None

    @field_validator("question")
    @classmethod
    def validate_non_empty_question(cls, v: str) -> str:
        stripped = v.strip()
        if not stripped:
            raise ValueError("Question must not be empty or whitespace only")
        return stripped
