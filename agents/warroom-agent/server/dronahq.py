import json
import logging
import httpx
from server.config import settings
from server.schemas import (
    AskWarroomEvidence,
    AskWarroomResponse,
    CompanyContext,
    CompetitiveSignal,
    CompetitorResearch,
    DepartmentImpact,
    DronaHQRun,
    MarketingResponse,
    ProductResponse,
    RelevantSignalRef,
    ResponseSimulation,
    SalesResponse,
    SignalContextSummary,
    SignalOverlap,
    SimulationEvidence,
    StrategyResponse,
)

logger = logging.getLogger("warroom-dronahq")


class DronaHQError(Exception):
    """Base exception for DronaHQ client operations."""


class DronaHQConfigurationError(DronaHQError):
    """Raised when DronaHQ webhook URL or API key is not configured."""


class DronaHQTimeoutError(DronaHQError):
    """Raised when DronaHQ webhook trigger request times out."""


class DronaHQAPIError(DronaHQError):
    """Raised when DronaHQ webhook returns an HTTP error status."""

    def __init__(self, status_code: int, detail: str):
        super().__init__(f"DronaHQ API returned {status_code}: {detail}")
        self.status_code = status_code
        self.detail = detail


class DronaHQResponseError(DronaHQError):
    """Raised when DronaHQ response cannot be parsed."""


def _clean_json_string(s: str) -> str:
    """Strip markdown code fences (e.g. ```json ... ```) from LLM output."""
    cleaned = s.strip()
    if cleaned.startswith("```json"):
        cleaned = cleaned[7:]
    elif cleaned.startswith("```"):
        cleaned = cleaned[3:]
    if cleaned.endswith("```"):
        cleaned = cleaned[:-3]
    return cleaned.strip()


def _normalize_signal_item(
    item: str | dict,
    known_competitors: list[str],
    research_by_competitor: dict[str, list[str]],
    top_level_context: dict | None = None,
) -> CompetitiveSignal | None:
    """Normalize a raw signal item (dict or string) into a CompetitiveSignal model."""
    if isinstance(item, dict):
        comp = item.get("competitor")
        if not comp:
            # Try to infer competitor from headline or summary
            combined = f"{item.get('headline', '')} {item.get('summary', '')} {item.get('change', '')} {item.get('changed', '')}"
            for c in known_competitors:
                if c.lower() in combined.lower():
                    comp = c
                    break
            comp = comp or "Competitive Market"

        category = item.get("category") or item.get("signal_type")
        if not category:
            sig_class = item.get("signal_classification") or item.get("signalClassification")
            if isinstance(sig_class, dict):
                category = sig_class.get("type")

        headline = item.get("headline") or item.get("change") or item.get("changed") or item.get("title")
        summary = (
            item.get("summary")
            or item.get("significance_explanation")
            or item.get("significanceExplanation")
            or headline
        )

        sources = item.get("source_urls") or research_by_competitor.get(comp, [])

        # Extract significance (interpretation / classification)
        significance = item.get("significance")
        if not significance:
            sig_class = item.get("signal_classification") or item.get("signalClassification")
            if isinstance(sig_class, dict):
                significance = sig_class.get("significance") or sig_class.get("threat_level")
            elif isinstance(sig_class, str):
                significance = sig_class

        # Extract product & customer overlap
        overlap_dict = item.get("overlap") if isinstance(item.get("overlap"), dict) else {}
        raw_products = (
            overlap_dict.get("products")
            or item.get("affected_products")
            or item.get("affected_internal_products")
            or []
        )
        products = [str(p) for p in raw_products if p] if isinstance(raw_products, list) else []

        raw_segments = (
            overlap_dict.get("customer_segments")
            or item.get("affected_customer_segments")
            or item.get("affected_segments")
            or item.get("customer_segments")
            or []
        )
        customer_segments = [str(s) for s in raw_segments if s] if isinstance(raw_segments, list) else []
        overlap = SignalOverlap(products=products, customer_segments=customer_segments)

        # Extract departmental impact
        raw_impact = item.get("impact") or item.get("impact_assessment") or {}
        if isinstance(raw_impact, dict):
            impact = DepartmentImpact(
                product=str(raw_impact["product"]) if raw_impact.get("product") else None,
                sales=str(raw_impact["sales"]) if raw_impact.get("sales") else None,
                marketing=str(raw_impact["marketing"]) if raw_impact.get("marketing") else None,
                strategy=str(raw_impact["strategy"]) if raw_impact.get("strategy") else (str(raw_impact["pricing"]) if raw_impact.get("pricing") else None),
            )
        else:
            impact = DepartmentImpact()

        # Extract recommended actions (investigation, sales talk tracks, considerations)
        raw_actions = item.get("recommended_actions")
        actions: list[str] = []
        if isinstance(raw_actions, list):
            actions = [str(a) for a in raw_actions if a]
        elif isinstance(raw_actions, str) and raw_actions.strip():
            actions = [raw_actions.strip()]

        if not actions:
            rec = item.get("product_investigation_recommendation") or (
                top_level_context.get("product_investigation_recommendation")
                if top_level_context
                else None
            )
            if rec and isinstance(rec, str):
                actions.append(rec.strip())
            battlecard = item.get("sales_battlecard") or (
                top_level_context.get("sales_battlecard")
                if top_level_context
                else None
            )
            if isinstance(battlecard, dict):
                talk_track = battlecard.get("recommended_talk_track")
                if talk_track:
                    actions.append(f"Sales talk track: {talk_track}")

        # Extract confidence rating based on available evidence
        raw_conf = item.get("confidence")
        if isinstance(raw_conf, str) and raw_conf.lower() in ("high", "medium", "low"):
            confidence = raw_conf.lower()
        else:
            confidence = "high" if sources else "medium"

        return CompetitiveSignal(
            competitor=str(comp),
            category=str(category) if category else "product",
            headline=str(headline) if headline else (str(summary)[:100] if summary else None),
            summary=str(summary) if summary else str(headline),
            significance=str(significance) if significance else None,
            overlap=overlap,
            impact=impact,
            recommended_actions=actions,
            confidence=confidence,
            source_urls=sources,
        )

    if isinstance(item, str):
        text = _clean_json_string(item)
        if not text:
            return None

        # Check if the string is stringified JSON
        if (text.startswith("{") and text.endswith("}")) or (text.startswith("[") and text.endswith("]")):
            try:
                parsed = json.loads(text)
                if isinstance(parsed, dict):
                    return _normalize_signal_item(parsed, known_competitors, research_by_competitor)
                if isinstance(parsed, list) and parsed:
                    return _normalize_signal_item(parsed[0], known_competitors, research_by_competitor)
            except Exception:
                pass

        # Identify competitor from text
        matched_comp = "Competitive Market"
        for c in known_competitors:
            if c.lower() in text.lower():
                matched_comp = c
                break

        # Extract headline and summary
        lines = [line.strip() for line in text.split("\n") if line.strip()]
        headline = lines[0] if lines else text
        if len(headline) > 120:
            period_idx = headline.find(". ")
            if 0 < period_idx <= 120:
                headline = headline[: period_idx + 1]
            else:
                headline = headline[:117] + "..."

        source_urls = research_by_competitor.get(matched_comp, [])

        return CompetitiveSignal(
            competitor=matched_comp,
            category="product",
            headline=headline,
            summary=text,
            significance=None,
            overlap=SignalOverlap(),
            impact=DepartmentImpact(),
            recommended_actions=[],
            confidence="medium" if source_urls else "low",
            source_urls=source_urls,
        )

    return None


async def _post_dronahq_webhook(payload: dict) -> dict:
    """Send payload to DronaHQ webhook and return parsed response dictionary."""
    webhook_url = settings.dronahq_webhook_url
    api_key = settings.dronahq_api_key

    if not webhook_url or not webhook_url.strip():
        raise DronaHQConfigurationError("DRONAHQ_WEBHOOK_URL is not configured in the environment")
    if not api_key or not api_key.strip():
        raise DronaHQConfigurationError("DRONAHQ_API_KEY is not configured in the environment")

    headers = {
        "Content-Type": "application/json",
        "api-key": api_key.strip(),
    }

    try:
        async with httpx.AsyncClient(timeout=45.0) as client:
            response = await client.post(webhook_url.strip(), headers=headers, json=payload)
    except httpx.TimeoutException as exc:
        raise DronaHQTimeoutError("DronaHQ webhook request timed out after 45.0s") from exc
    except httpx.RequestError as exc:
        raise DronaHQAPIError(status_code=500, detail=f"Network error communicating with DronaHQ: {exc}") from exc

    if response.status_code not in (200, 201, 202):
        raise DronaHQAPIError(status_code=response.status_code, detail=response.text)

    try:
        data = response.json()
    except Exception as exc:
        raise DronaHQResponseError(f"Invalid JSON received from DronaHQ webhook: {exc}") from exc

    if not isinstance(data, dict):
        raise DronaHQResponseError(f"Expected dictionary from DronaHQ, got {type(data).__name__}")

    return data


async def trigger_reasoning(
    payload: dict,
    research_context: list[CompetitorResearch] | None = None,
) -> tuple[DronaHQRun, list[CompetitiveSignal]]:
    """Trigger the DronaHQ reasoning agent via its configured webhook URL
    and parse the Standard response containing structured competitive signals.

    Verified endpoint: POST <DRONAHQ_WEBHOOK_URL>
    Headers: Content-Type: application/json, api-key: <DRONAHQ_API_KEY>
    """
    data = await _post_dronahq_webhook(payload)

    success = data.get("success")
    thread_id = data.get("thread_id")
    run_id = data.get("run_id")
    message = data.get("message")
    raw_response = data.get("response")
    logger.info("DronaHQ full response payload: %r", data)
    logger.info("DronaHQ raw_response received (type=%s): %r", type(raw_response).__name__, raw_response)

    # Map sources by competitor for URL linkage
    known_competitors: list[str] = []
    research_by_competitor: dict[str, list[str]] = {}
    if research_context:
        for cr in research_context:
            known_competitors.append(cr.competitor)
            research_by_competitor[cr.competitor] = [r.url for r in cr.results if r.url]

    # Case 1: Standard response with output payload in "response"
    if success and raw_response:
        parsed_output: dict = {}
        if isinstance(raw_response, dict):
            parsed_output = raw_response
        elif isinstance(raw_response, str):
            cleaned_str = _clean_json_string(raw_response)
            try:
                parsed_output = json.loads(cleaned_str)
            except Exception:
                logger.warning("DronaHQ response was a plain text string rather than JSON: %s", raw_response[:100])
                parsed_output = {"status": "success", "signals": [raw_response]}

        output_status = parsed_output.get("status", "success") if isinstance(parsed_output, dict) else "success"
        if output_status == "insufficient_evidence":
            logger.info("DronaHQ reported insufficient evidence to extract competitive signals")
            dronahq_run = DronaHQRun(
                status="insufficient_evidence",
                thread_id=str(thread_id) if thread_id is not None else None,
                run_id=str(run_id) if run_id is not None else None,
                message=str(message) if message is not None else "Agent completed with insufficient evidence.",
            )
            return dronahq_run, []

        signals: list[CompetitiveSignal] = []
        if isinstance(parsed_output, dict):
            # Check for "competitive_signals", "competitor_signals", "competitorSignals", or "signals"
            raw_signals = (
                parsed_output.get("competitive_signals")
                or parsed_output.get("competitor_signals")
                or parsed_output.get("competitorSignals")
                or parsed_output.get("signals", [])
            )
            if isinstance(raw_signals, list):
                for item in raw_signals:
                    normalized = _normalize_signal_item(
                        item,
                        known_competitors,
                        research_by_competitor,
                        top_level_context=parsed_output,
                    )
                    if normalized:
                        signals.append(normalized)
        elif isinstance(parsed_output, list):
            for item in parsed_output:
                normalized = _normalize_signal_item(item, known_competitors, research_by_competitor)
                if normalized:
                    signals.append(normalized)

        dronahq_run = DronaHQRun(
            status="completed",
            thread_id=str(thread_id) if thread_id is not None else None,
            run_id=str(run_id) if run_id is not None else None,
            message=str(message) if message is not None else "Agent run completed successfully.",
        )
        return dronahq_run, signals

    # Case 2: Asynchronous background acknowledgement (pending)
    if success and not raw_response:
        dronahq_run = DronaHQRun(
            status="pending",
            thread_id=str(thread_id) if thread_id is not None else None,
            run_id=str(run_id) if run_id is not None else None,
            message=str(message) if message is not None else "Agent run started in background.",
        )
        return dronahq_run, []

    # Case 3: Error
    dronahq_run = DronaHQRun(
        status="error",
        thread_id=str(thread_id) if thread_id is not None else None,
        run_id=str(run_id) if run_id is not None else None,
        message=str(message) if message is not None else "DronaHQ run failed to start.",
    )
    return dronahq_run, []


def _extract_list(val) -> list[str]:
    """Helper to extract a clean list of strings from arbitrary JSON input."""
    if isinstance(val, list):
        return [str(item).strip() for item in val if item and str(item).strip()]
    if isinstance(val, str) and val.strip():
        return [val.strip()]
    return []


def _build_fallback_simulation(
    signal: CompetitiveSignal,
    company: CompanyContext,
    reason: str | None = None,
) -> ResponseSimulation:
    """Construct an evidence-grounded fallback ResponseSimulation when DronaHQ
    reasoning is unavailable or returns an invalid payload.

    Preserves original signal context, source URLs, and existing recommended actions
    without fabricating facts or internal capabilities.
    """
    competitor = signal.competitor
    headline = signal.headline or f"Activity from {competitor}"
    products = signal.overlap.products or company.products
    segments = signal.overlap.customer_segments or company.target_customers

    # Product actions & investigation
    prod_actions: list[str] = []
    if signal.recommended_actions:
        prod_actions.extend([a for a in signal.recommended_actions if "sales" not in a.lower()])
    if not prod_actions:
        target_prods = ", ".join(products[:2]) if products else "core payment solutions"
        prod_actions = [
            f"Investigate whether PayFlow's {target_prods} capabilities address challenges highlighted by {competitor}'s {headline}."
        ]

    prod_questions = [
        f"Which specific capabilities announced by {competitor} are verified in primary documentation?",
        f"Does this competitor move impact {', '.join(segments) if segments else 'target customer'} adoption in the next 1-2 quarters?",
    ]
    prod_caveats = [
        "Competitor capabilities and performance claims must be validated against primary sources.",
    ]

    # Sales triggers, talk tracks, questions
    sales_trigger = f"Prospect or customer references {competitor}'s {headline} during evaluation."
    # Check if a sales talk track is already in recommended_actions
    sales_talk_track = None
    for a in signal.recommended_actions:
        if "sales talk track:" in a.lower():
            sales_talk_track = a.split(":", 1)[1].strip()
            break
    if not sales_talk_track:
        target_segs = ", ".join(segments) if segments else "our merchant base"
        sales_talk_track = (
            f"Acknowledge {competitor}'s announcement, then re-anchor on PayFlow's proven reliability and dedicated support for {target_segs}."
        )

    sales_questions = [
        f"Which specific operational or fraud/settlement concerns prompted the comparison with {competitor}?",
    ]
    sales_caveats = [
        "Do not claim feature parity or make unsupported comparisons without engineering verification.",
    ]

    # Marketing actions, messaging, caveats
    target_prods = ", ".join(products[:2]) if products else "core products"
    target_segs = ", ".join(segments) if segments else "merchants"
    mktg_actions = [
        f"Audit current PayFlow positioning for {target_prods} to ensure value proposition is clearly communicated.",
    ]
    mktg_messaging = [
        f"Emphasize PayFlow's merchant-focused workflow and transparent pricing for {target_segs}.",
    ]
    mktg_caveats = [
        "Avoid reactionary or unsupported superiority claims in marketing materials.",
    ]

    # Strategy questions, actions, caveats
    strat_questions = [
        f"Does {competitor}'s {headline} represent a broader category trend or an isolated announcement?",
        "Should PayFlow prioritize proactive roadmap investment or passive competitive monitoring?",
    ]
    strat_actions = [
        f"Track win/loss feedback in the sales pipeline when {competitor} is cited by prospects.",
    ]
    strat_caveats = [
        "Decision support recommendation only; validate with sales and customer data before committing roadmap resources.",
    ]

    # Grounding check: verify source URLs
    if not signal.source_urls:
        confidence = "low"
        evidence_note = "Evidence is insufficient: no verified source URLs provided in competitive signal."
        prod_caveats.append(evidence_note)
        sales_caveats.append(evidence_note)
        mktg_caveats.append(evidence_note)
        strat_caveats.append(evidence_note)
    else:
        confidence = signal.confidence or "medium"

    if reason:
        prod_caveats.insert(0, f"AI simulation fallback: {reason}")

    return ResponseSimulation(
        signal_context=SignalContextSummary(
            competitor=signal.competitor,
            headline=signal.headline,
            category=signal.category,
            significance=signal.significance,
        ),
        product=ProductResponse(
            actions=prod_actions,
            investigation_questions=prod_questions,
            caveats=prod_caveats,
        ),
        sales=SalesResponse(
            trigger=sales_trigger,
            talk_track=sales_talk_track,
            questions=sales_questions,
            caveats=sales_caveats,
        ),
        marketing=MarketingResponse(
            actions=mktg_actions,
            messaging_angles=mktg_messaging,
            caveats=mktg_caveats,
        ),
        strategy=StrategyResponse(
            questions=strat_questions,
            actions=strat_actions,
            caveats=strat_caveats,
        ),
        evidence=SimulationEvidence(source_urls=list(signal.source_urls)),
        confidence=confidence,
    )


def _normalize_simulation_output(
    raw_output: dict | str,
    signal: CompetitiveSignal,
    company: CompanyContext,
) -> ResponseSimulation:
    """Normalize raw DronaHQ reasoning output into a validated ResponseSimulation model.

    Tolerates JSON strings, markdown fences, dictionary wrappers, camelCase variations,
    and blends with existing signal data when fields are sparse. Strictly preserves source URLs.
    """
    parsed: dict = {}
    if isinstance(raw_output, str):
        cleaned = _clean_json_string(raw_output)
        try:
            parsed = json.loads(cleaned)
        except Exception:
            logger.warning("Simulation response could not be parsed as JSON: %s", raw_output[:100])
            return _build_fallback_simulation(signal, company, reason="AI returned unparseable text")
    elif isinstance(raw_output, dict):
        parsed = raw_output
    else:
        return _build_fallback_simulation(signal, company, reason="Invalid response type received from AI")

    # Unwrap wrappers if present
    if isinstance(parsed.get("response"), dict):
        parsed = parsed["response"]
    elif isinstance(parsed.get("simulation"), dict):
        parsed = parsed["simulation"]
    elif isinstance(parsed.get("response_simulation"), dict):
        parsed = parsed["response_simulation"]

    if parsed.get("status") == "insufficient_evidence":
        return _build_fallback_simulation(signal, company, reason="Insufficient evidence reported by reasoning agent")

    # 1. Product
    raw_prod = parsed.get("product") or parsed.get("product_response") or {}
    if not isinstance(raw_prod, dict):
        raw_prod = {}
    prod_actions = _extract_list(raw_prod.get("actions") or raw_prod.get("recommendations"))
    prod_questions = _extract_list(
        raw_prod.get("investigation_questions")
        or raw_prod.get("investigationQuestions")
        or raw_prod.get("questions")
    )
    prod_caveats = _extract_list(raw_prod.get("caveats") or raw_prod.get("limitations"))

    # 2. Sales
    raw_sales = parsed.get("sales") or parsed.get("sales_response") or {}
    if not isinstance(raw_sales, dict):
        raw_sales = {}
    sales_trigger = raw_sales.get("trigger") or raw_sales.get("triggers")
    if isinstance(sales_trigger, list) and sales_trigger:
        sales_trigger = str(sales_trigger[0])
    elif sales_trigger:
        sales_trigger = str(sales_trigger)

    sales_talk_track = (
        raw_sales.get("talk_track")
        or raw_sales.get("talkTrack")
        or raw_sales.get("recommended_talk_track")
    )
    if isinstance(sales_talk_track, list) and sales_talk_track:
        sales_talk_track = str(sales_talk_track[0])
    elif sales_talk_track:
        sales_talk_track = str(sales_talk_track)

    sales_questions = _extract_list(raw_sales.get("questions") or raw_sales.get("investigation_questions"))
    sales_caveats = _extract_list(raw_sales.get("caveats") or raw_sales.get("limitations"))

    # 3. Marketing
    raw_mktg = parsed.get("marketing") or parsed.get("marketing_response") or {}
    if not isinstance(raw_mktg, dict):
        raw_mktg = {}
    mktg_actions = _extract_list(raw_mktg.get("actions") or raw_mktg.get("recommendations"))
    mktg_messaging = _extract_list(
        raw_mktg.get("messaging_angles")
        or raw_mktg.get("messagingAngles")
        or raw_mktg.get("angles")
    )
    mktg_caveats = _extract_list(raw_mktg.get("caveats") or raw_mktg.get("limitations"))

    # 4. Strategy
    raw_strat = parsed.get("strategy") or parsed.get("strategic") or parsed.get("strategy_response") or {}
    if not isinstance(raw_strat, dict):
        raw_strat = {}
    strat_questions = _extract_list(
        raw_strat.get("questions")
        or raw_strat.get("strategic_questions")
        or raw_strat.get("strategicQuestions")
    )
    strat_actions = _extract_list(raw_strat.get("actions") or raw_strat.get("recommendations"))
    strat_caveats = _extract_list(raw_strat.get("caveats") or raw_strat.get("limitations"))

    # 5. Confidence
    raw_conf = parsed.get("confidence")
    if isinstance(raw_conf, str) and raw_conf.lower() in ("high", "medium", "low"):
        confidence = raw_conf.lower()
    else:
        confidence = signal.confidence or ("medium" if signal.source_urls else "low")

    # Blend with fallback defaults if sections are sparse
    fallback = _build_fallback_simulation(signal, company)
    if not prod_actions:
        prod_actions = fallback.product.actions
    if not prod_questions:
        prod_questions = fallback.product.investigation_questions
    if not prod_caveats:
        prod_caveats = fallback.product.caveats

    if not sales_trigger:
        sales_trigger = fallback.sales.trigger
    if not sales_talk_track:
        sales_talk_track = fallback.sales.talk_track
    if not sales_questions:
        sales_questions = fallback.sales.questions
    if not sales_caveats:
        sales_caveats = fallback.sales.caveats

    if not mktg_actions:
        mktg_actions = fallback.marketing.actions
    if not mktg_messaging:
        mktg_messaging = fallback.marketing.messaging_angles
    if not mktg_caveats:
        mktg_caveats = fallback.marketing.caveats

    if not strat_questions:
        strat_questions = fallback.strategy.questions
    if not strat_actions:
        strat_actions = fallback.strategy.actions
    if not strat_caveats:
        strat_caveats = fallback.strategy.caveats

    # Strictly preserve signal source URLs
    evidence_urls = list(signal.source_urls)
    raw_evidence = parsed.get("evidence")
    if isinstance(raw_evidence, dict):
        extra_urls = raw_evidence.get("source_urls") or []
        if isinstance(extra_urls, list):
            for u in extra_urls:
                if u and isinstance(u, str) and u not in evidence_urls:
                    evidence_urls.append(u)
    elif isinstance(raw_evidence, list):
        for u in raw_evidence:
            if u and isinstance(u, str) and u not in evidence_urls:
                evidence_urls.append(u)

    # If source URLs are empty, enforce low confidence and caveats
    if not evidence_urls:
        confidence = "low"
        insufficient_note = "Evidence is insufficient: no verified source URLs provided in competitive signal."
        for cav_list in [prod_caveats, sales_caveats, mktg_caveats, strat_caveats]:
            if insufficient_note not in cav_list:
                cav_list.append(insufficient_note)

    return ResponseSimulation(
        signal_context=SignalContextSummary(
            competitor=signal.competitor,
            headline=signal.headline,
            category=signal.category,
            significance=signal.significance,
        ),
        product=ProductResponse(
            actions=prod_actions,
            investigation_questions=prod_questions,
            caveats=prod_caveats,
        ),
        sales=SalesResponse(
            trigger=sales_trigger,
            talk_track=sales_talk_track,
            questions=sales_questions,
            caveats=sales_caveats,
        ),
        marketing=MarketingResponse(
            actions=mktg_actions,
            messaging_angles=mktg_messaging,
            caveats=mktg_caveats,
        ),
        strategy=StrategyResponse(
            questions=strat_questions,
            actions=strat_actions,
            caveats=strat_caveats,
        ),
        evidence=SimulationEvidence(source_urls=evidence_urls),
        confidence=confidence,
    )


async def simulate_response(
    signal: CompetitiveSignal,
    company: CompanyContext | None = None,
) -> ResponseSimulation:
    """Generate structured response options across Product, Sales, Marketing, and Strategy
    for a given CompetitiveSignal using DronaHQ reasoning, with resilient fallback.
    """
    from server.fallback import load_default_company_context

    if company is None:
        company, _ = load_default_company_context()

    prompt_message = (
        f"Generate decision-support response options for {company.name} regarding a competitive move by {signal.competitor}.\n\n"
        "You are generating response OPTIONS for investigation and decision support.\n"
        "Do not make the decision for the user.\n"
        "Do not invent facts.\n"
        "Use only information contained in the supplied signal and evidence.\n"
        "Clearly distinguish:\n"
        "- known evidence\n"
        "- reasonable response option\n"
        "- question requiring validation\n"
        f"Do not claim that {company.name} has a capability unless it is present in the supplied company context.\n"
        "Do not claim the competitor has a capability unless supported by the supplied evidence.\n\n"
        "Output a JSON object with keys: product, sales, marketing, strategy, confidence.\n"
        "Schema requirements:\n"
        "- product: { actions: [str], investigation_questions: [str], caveats: [str] }\n"
        "- sales: { trigger: str, talk_track: str, questions: [str], caveats: [str] }\n"
        "- marketing: { actions: [str], messaging_angles: [str], caveats: [str] }\n"
        "- strategy: { questions: [str], actions: [str], caveats: [str] }\n"
        "- confidence: 'high' | 'medium' | 'low'\n"
    )

    dronahq_payload = {
        "message": prompt_message,
        "company": {
            "name": company.name,
            "products": company.products,
            "target_customers": company.target_customers,
        },
        "signal": {
            "competitor": signal.competitor,
            "category": signal.category,
            "headline": signal.headline,
            "summary": signal.summary,
            "significance": signal.significance,
            "overlap": {
                "products": signal.overlap.products,
                "customer_segments": signal.overlap.customer_segments,
            },
            "impact": {
                "product": signal.impact.product,
                "sales": signal.impact.sales,
                "marketing": signal.impact.marketing,
                "strategy": signal.impact.strategy,
            },
            "recommended_actions": signal.recommended_actions,
            "confidence": signal.confidence,
            "source_urls": signal.source_urls,
        },
    }

    try:
        data = await _post_dronahq_webhook(dronahq_payload)
        raw_response = data.get("response")
        logger.info("DronaHQ simulation response received: %r", raw_response)
        if data.get("success") and raw_response:
            return _normalize_simulation_output(raw_response, signal, company)
        else:
            logger.warning("DronaHQ returned success=%s with message=%s", data.get("success"), data.get("message"))
            return _build_fallback_simulation(signal, company, reason=data.get("message") or "Async acknowledgment received")
    except (DronaHQConfigurationError, DronaHQTimeoutError, DronaHQAPIError, DronaHQResponseError) as exc:
        logger.warning("DronaHQ reasoning error during simulation, utilizing fallback response: %s", exc)
        return _build_fallback_simulation(signal, company, reason=str(exc))
    except Exception as exc:
        logger.error("Unexpected error during simulation: %s", exc)
        return _build_fallback_simulation(signal, company, reason=f"Unexpected error: {exc}")


def _build_fallback_ask_response(
    question: str,
    signals: list[CompetitiveSignal],
    company: CompanyContext,
    reason: str | None = None,
) -> AskWarroomResponse:
    """Build a deterministic, evidence-grounded fallback AskWarroomResponse
    when DronaHQ reasoning fails, times out, or returns unparseable text.
    """
    if not signals:
        return AskWarroomResponse(
            answer="Insufficient context: No competitive signals were provided to answer this question.",
            key_points=[],
            relevant_signals=[],
            evidence=AskWarroomEvidence(source_urls=[]),
            confidence="low",
            limitations=["No active competitive signals were provided in the request context."],
        )

    # Determine relevant signals deterministically by keyword matching
    question_lower = question.lower()
    matched_signals: list[CompetitiveSignal] = []
    for s in signals:
        if s.competitor.lower() in question_lower:
            matched_signals.append(s)
        elif s.headline and any(w in question_lower for w in s.headline.lower().split() if len(w) > 4):
            matched_signals.append(s)

    # If no specific signal matched, treat all supplied signals as relevant (e.g. landscape query)
    if not matched_signals:
        matched_signals = signals

    key_points: list[str] = []
    for s in matched_signals[:4]:
        point = f"{s.competitor}: {s.headline or 'Recent competitive activity'}"
        if s.significance:
            point += f" (Significance: {s.significance})"
        key_points.append(point)

    relevant_refs = [
        RelevantSignalRef(competitor=s.competitor, headline=s.headline)
        for s in matched_signals
    ]

    # Gather source URLs strictly from matched signals
    evidence_urls: list[str] = []
    for s in matched_signals:
        for u in s.source_urls:
            if u and u not in evidence_urls:
                evidence_urls.append(u)

    reason_str = f" ({reason})" if reason else ""
    answer = (
        f"AI reasoning is currently unavailable{reason_str}. "
        f"Context contains {len(signals)} competitive signal(s) evaluated for {company.name}. "
        f"Relevant activity detected for: {', '.join({s.competitor for s in matched_signals})}."
    )

    limitations = [
        "AI reasoning service unavailable; response generated from signal heuristics.",
        "Synthesized without deep semantic analysis.",
    ]
    if not evidence_urls:
        limitations.append("Insufficient evidence: no verified source URLs available in the supplied signals.")

    return AskWarroomResponse(
        answer=answer,
        key_points=key_points,
        relevant_signals=relevant_refs,
        evidence=AskWarroomEvidence(source_urls=evidence_urls),
        confidence="low",
        limitations=limitations,
    )


def _normalize_ask_response(
    raw_output: dict | str,
    question: str,
    signals: list[CompetitiveSignal],
    company: CompanyContext,
) -> AskWarroomResponse:
    """Normalize raw DronaHQ reasoning output into a validated AskWarroomResponse model.
    Strictly filters evidence URLs against the supplied CompetitiveSignal source URLs.
    """
    parsed: dict = {}
    if isinstance(raw_output, str):
        cleaned = _clean_json_string(raw_output)
        try:
            parsed = json.loads(cleaned)
        except Exception:
            logger.warning("Ask WARROOM response could not be parsed as JSON: %s", raw_output[:100])
            if cleaned and len(cleaned) > 20:
                all_supplied_urls = [u for s in signals for u in s.source_urls if u]
                bullets = [l.strip("- *•") for l in cleaned.splitlines() if l.strip().startswith(("-", "*", "•"))]
                matched_refs = [
                    RelevantSignalRef(competitor=s.competitor, headline=s.headline)
                    for s in signals
                    if s.competitor.lower() in question.lower() or s.competitor.lower() in cleaned.lower()
                ] or [RelevantSignalRef(competitor=s.competitor, headline=s.headline) for s in signals[:2]]
                lims = ["Response formatted as plain text by reasoning agent."]
                if not all_supplied_urls:
                    lims.append("Insufficient evidence: no verified source URLs available in the supplied competitive context.")
                return AskWarroomResponse(
                    answer=cleaned,
                    key_points=_extract_list(bullets),
                    relevant_signals=matched_refs,
                    evidence=AskWarroomEvidence(source_urls=all_supplied_urls[:3]),
                    confidence="low" if not all_supplied_urls else "medium",
                    limitations=lims,
                )
            return _build_fallback_ask_response(question, signals, company, reason="AI returned unparseable text")
    elif isinstance(raw_output, dict):
        parsed = raw_output
    else:
        return _build_fallback_ask_response(question, signals, company, reason="Invalid response type received from AI")

    # Unwrap wrappers if present
    if isinstance(parsed.get("response"), dict):
        parsed = parsed["response"]
    elif isinstance(parsed.get("ask_response"), dict):
        parsed = parsed["ask_response"]
    elif isinstance(parsed.get("data"), dict):
        parsed = parsed["data"]

    # 1. Answer
    raw_answer = (
        parsed.get("answer")
        or parsed.get("interpretation")
        or parsed.get("significance_explanation")
        or parsed.get("summary")
        or (parsed.get("response") if isinstance(parsed.get("response"), str) else None)
    )
    if isinstance(raw_answer, dict):
        # Extract fields like importance, potential_impact or concatenate all string values
        parts = []
        if raw_answer.get("importance"):
            parts.append(str(raw_answer["importance"]).strip())
        if raw_answer.get("potential_impact"):
            parts.append(str(raw_answer["potential_impact"]).strip())
        if not parts:
            parts = [str(v).strip() for v in raw_answer.values() if v and isinstance(v, str)]
        answer = " ".join(parts)
    elif isinstance(raw_answer, str) and raw_answer.strip():
        answer = raw_answer.strip()
    else:
        answer = ""

    if not answer:
        fallback = _build_fallback_ask_response(question, signals, company, reason="AI returned empty answer")
        answer = fallback.answer

    # 2. Key points
    key_points = _extract_list(
        parsed.get("key_points")
        or parsed.get("keyPoints")
        or parsed.get("highlights")
        or parsed.get("takeaways")
        or parsed.get("recommended_investigation")
        or parsed.get("recommendations")
    )
    if not key_points and isinstance(parsed.get("recommendations"), dict):
        rec_dict = parsed["recommendations"]
        for k, v in rec_dict.items():
            if isinstance(v, list):
                key_points.extend(_extract_list(v))
            elif isinstance(v, str) and v.strip():
                key_points.append(v.strip())

    if not key_points:
        raw_rationale = parsed.get("impact_rationale")
        if isinstance(raw_rationale, dict):
            key_points = [
                f"{dept.capitalize()} Impact: {text}"
                for dept, text in raw_rationale.items()
                if text and isinstance(text, str)
            ]
        elif raw_strat := parsed.get("strategic_questions"):
            key_points = _extract_list(raw_strat)

    # 3. Relevant signals
    raw_relevant = (
        parsed.get("relevant_signals")
        or parsed.get("relevantSignals")
        or parsed.get("signals")
    )
    relevant_signals: list[RelevantSignalRef] = []
    if isinstance(raw_relevant, list):
        for item in raw_relevant:
            if isinstance(item, dict):
                comp = item.get("competitor") or item.get("name")
                headline = item.get("headline") or item.get("title")
                if comp:
                    relevant_signals.append(
                        RelevantSignalRef(
                            competitor=str(comp),
                            headline=str(headline) if headline else None,
                        )
                    )
            elif isinstance(item, str) and item.strip():
                comp_match = None
                for s in signals:
                    if s.competitor.lower() in item.lower():
                        comp_match = s
                        break
                if comp_match:
                    relevant_signals.append(
                        RelevantSignalRef(competitor=comp_match.competitor, headline=comp_match.headline)
                    )
                else:
                    relevant_signals.append(RelevantSignalRef(competitor=item.strip()))

    if not relevant_signals and isinstance(parsed.get("verified_evidence"), list):
        for ve_item in parsed["verified_evidence"]:
            if isinstance(ve_item, dict):
                comp = ve_item.get("competitor")
                change_desc = ve_item.get("changed") or ve_item.get("change") or ve_item.get("headline")
                if comp:
                    relevant_signals.append(
                        RelevantSignalRef(
                            competitor=str(comp),
                            headline=str(change_desc) if change_desc else None,
                        )
                    )
    elif not relevant_signals and isinstance(parsed.get("verified_evidence"), dict):
        ve = parsed["verified_evidence"]
        comp = ve.get("competitor")
        change_desc = ve.get("change") or ve.get("headline")
        if comp:
            relevant_signals.append(RelevantSignalRef(competitor=str(comp), headline=str(change_desc) if change_desc else None))

    if not relevant_signals and parsed.get("competitor"):
        comp_name = str(parsed["competitor"])
        change_desc = parsed.get("change") or parsed.get("headline")
        relevant_signals.append(RelevantSignalRef(competitor=comp_name, headline=str(change_desc) if change_desc else None))

    if not relevant_signals:
        for s in signals:
            if s.competitor.lower() in question.lower() or (
                s.headline and any(w in question.lower() for w in s.headline.lower().split() if len(w) > 4)
            ):
                relevant_signals.append(RelevantSignalRef(competitor=s.competitor, headline=s.headline))
        if not relevant_signals and signals:
            relevant_signals = [RelevantSignalRef(competitor=s.competitor, headline=s.headline) for s in signals]

    # 4. Evidence filtering — strictly preserve ONLY supplied source URLs
    supplied_urls_set: set[str] = set()
    for s in signals:
        for u in s.source_urls:
            if u:
                supplied_urls_set.add(u)

    raw_evidence = parsed.get("evidence")
    unfiltered_urls: list[str] = []
    if isinstance(raw_evidence, dict):
        unfiltered_urls = _extract_list(raw_evidence.get("source_urls") or raw_evidence.get("urls"))
    elif isinstance(raw_evidence, list):
        unfiltered_urls = _extract_list(raw_evidence)

    if isinstance(parsed.get("verified_evidence"), list):
        for ve_item in parsed["verified_evidence"]:
            if isinstance(ve_item, dict) and ve_item.get("source_url"):
                unfiltered_urls.append(str(ve_item["source_url"]).strip())
    elif isinstance(parsed.get("verified_evidence"), dict):
        unfiltered_urls.extend(_extract_list(parsed["verified_evidence"].get("sources") or []))

    # Strictly filter: keep ONLY URLs present in supplied_urls_set
    valid_evidence_urls = [u for u in unfiltered_urls if u in supplied_urls_set]

    # If DronaHQ didn't return valid URLs but relevant signals have URLs, attach matching URLs
    if not valid_evidence_urls:
        for ref in relevant_signals:
            for s in signals:
                if s.competitor.lower() == ref.competitor.lower():
                    for u in s.source_urls:
                        if u and u not in valid_evidence_urls:
                            valid_evidence_urls.append(u)

    # 5. Confidence
    raw_conf = parsed.get("confidence")
    if not supplied_urls_set:
        confidence = "low"
    elif isinstance(raw_conf, str) and raw_conf.lower() in ("high", "medium", "low"):
        confidence = raw_conf.lower()
    else:
        confidence = "medium"

    # 6. Limitations
    limitations = _extract_list(parsed.get("limitations") or parsed.get("caveats"))
    if not supplied_urls_set:
        no_src_note = "Insufficient evidence: no verified source URLs available in the supplied competitive context."
        if no_src_note not in limitations:
            limitations.append(no_src_note)

    return AskWarroomResponse(
        answer=answer,
        key_points=key_points,
        relevant_signals=relevant_signals,
        evidence=AskWarroomEvidence(source_urls=valid_evidence_urls),
        confidence=confidence,
        limitations=limitations,
    )


async def ask_warroom(
    question: str,
    signals: list[CompetitiveSignal],
    company: CompanyContext | None = None,
) -> AskWarroomResponse:
    """Answer a natural language question about current competitive signals
    using DronaHQ reasoning, strictly evidence-grounded, with resilient fallback.
    """
    from server.fallback import load_default_company_context

    if company is None:
        company, _ = load_default_company_context()

    # If no signals are provided, immediately return insufficient context
    if not signals:
        return AskWarroomResponse(
            answer="Insufficient context: No competitive signals were provided to answer this question.",
            key_points=[],
            relevant_signals=[],
            evidence=AskWarroomEvidence(source_urls=[]),
            confidence="low",
            limitations=["No active competitive signals were provided in the request context."],
        )

    # Build compact structured WARROOM context
    compact_signals = [
        {
            "competitor": s.competitor,
            "category": s.category,
            "headline": s.headline,
            "summary": s.summary,
            "significance": s.significance,
            "overlap": {
                "products": s.overlap.products,
                "customer_segments": s.overlap.customer_segments,
            },
            "impact": {
                "product": s.impact.product,
                "sales": s.impact.sales,
                "marketing": s.impact.marketing,
                "strategy": s.impact.strategy,
            },
            "recommended_actions": s.recommended_actions,
            "confidence": s.confidence,
            "source_urls": s.source_urls,
        }
        for s in signals
    ]

    prompt_message = (
        f"User Question: {question}\n\n"
        "You are WARROOM, a competitive intelligence reasoning assistant.\n\n"
        "Answer the user's question using ONLY the supplied company context, competitive signals, and evidence.\n\n"
        "Do not invent:\n"
        "- competitor facts\n"
        "- product capabilities\n"
        "- pricing\n"
        "- customer information\n"
        "- market share\n"
        "- dates\n"
        "- sources\n"
        "- URLs\n\n"
        "Distinguish clearly between:\n"
        "1. verified evidence\n"
        "2. interpretation of supplied evidence\n"
        "3. recommended investigation/questions\n\n"
        "Do not make strategic decisions for the user.\n"
        "When evidence is insufficient, say so explicitly.\n"
        "When referring to a competitive signal, preserve the supplied competitor and headline.\n"
        "When possible, cite the relevant supplied source URLs in the structured evidence field."
    )

    dronahq_payload = {
        "message": prompt_message,
        "company": {
            "name": company.name,
            "products": company.products,
            "target_customers": company.target_customers,
        },
        "competitors": [
            {
                "name": s.competitor,
                "research": [
                    {"title": s.headline or "Signal", "url": u, "snippet": s.summary or s.headline or ""}
                    for u in s.source_urls
                ] or [{"title": s.headline or "Signal", "url": "", "snippet": s.summary or ""}],
            }
            for s in signals
        ],
    }

    try:
        data = await _post_dronahq_webhook(dronahq_payload)
        raw_response = data.get("response")
        logger.info("DronaHQ Ask WARROOM response received: %r", raw_response)
        if data.get("success") and raw_response:
            return _normalize_ask_response(raw_response, question, signals, company)
        else:
            logger.warning("DronaHQ returned success=%s with message=%s", data.get("success"), data.get("message"))
            return _build_fallback_ask_response(question, signals, company, reason=data.get("message") or "Async acknowledgment received")
    except (DronaHQConfigurationError, DronaHQTimeoutError, DronaHQAPIError, DronaHQResponseError) as exc:
        logger.warning("DronaHQ reasoning error during Ask WARROOM, utilizing fallback response: %s", exc)
        return _build_fallback_ask_response(question, signals, company, reason=str(exc))
    except Exception as exc:
        logger.error("Unexpected error during Ask WARROOM: %s", exc)
        return _build_fallback_ask_response(question, signals, company, reason=f"Unexpected error: {exc}")
