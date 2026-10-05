import logging
from fastapi import FastAPI, HTTPException, Request, status
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse

from server.config import settings
from server.schemas import (
    AskWarroomRequest,
    AskWarroomResponse,
    CompanyContext,
    CompetitorResearch,
    DronaHQRun,
    ResponseSimulation,
    ScanRequest,
    ScanResponse,
    SimulateRequest,
)
from server.fallback import load_default_company_context
from server import anakin
from server import dronahq
from server import nasiko

# Configure logging without printing sensitive keys
logging.basicConfig(level=logging.INFO, format="%(asctime)s [%(levelname)s] %(name)s: %(message)s")
logger = logging.getLogger("warroom-api")

app = FastAPI(
    title="WARROOM API",
    description="AI Competitive Intelligence & Response Agent Backend",
    version="0.1.0",
)

# CORS configuration for Vite frontend development
app.add_middleware(
    CORSMiddleware,
    allow_origins=[
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        "http://localhost:3000",
        "http://127.0.0.1:3000",
    ],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)



@app.get("/api/health", status_code=status.HTTP_200_OK)
async def health_check():
    """Health check endpoint.

    Does NOT expose secrets or environment values.
    """
    return {
        "status": "ok",
        "service": "warroom-api",
    }


@app.post("/api/company", response_model=CompanyContext, status_code=status.HTTP_200_OK)
async def configure_company(company: CompanyContext):
    """Validate and echo company context without persistence."""
    logger.info("Company context received and validated: %s", company.name)
    return company


@app.post("/api/scan", response_model=ScanResponse, status_code=status.HTTP_200_OK)
async def scan_competitors(request: ScanRequest | None = None):
    """Scan competitive landscape across monitored competitors.

    Pipeline:
    1. Resolve company context & competitor targets (or use verified PayFlow defaults).
    2. Query live Anakin Search for each competitor with focused search prompt.
    3. Assemble structured research payload preserving source fields.
    4. Trigger DronaHQ reasoning webhook with combined research payload.
    5. Return research and DronaHQRun status ('pending' for async webhook).
    """
    default_company, default_competitors = load_default_company_context()

    company = request.company if (request and request.company) else default_company
    competitors = request.competitors if (request and request.competitors) else default_competitors

    logger.info("Initiating competitive scan for %s across %d competitors", company.name, len(competitors))

    # Step 1 & 2: Search research for each competitor via Anakin
    all_research: list[CompetitorResearch] = []

    for comp in competitors:
        # Construct focused search prompt: "<Competitor> <Products...> <Target Customers...>"
        parts = [comp.name]
        if company.products:
            parts.extend(company.products)
        if company.target_customers:
            parts.extend(company.target_customers)

        search_prompt = " ".join(parts)
        logger.info("Querying Anakin Search for competitor '%s' with prompt: '%s'", comp.name, search_prompt)

        try:
            results = await anakin.search(prompt=search_prompt, limit=settings.anakin_limit)
            all_research.append(CompetitorResearch(competitor=comp.name, results=results))
        except anakin.AnakinConfigurationError as exc:
            logger.error("Anakin configuration error: %s", exc)
            raise HTTPException(
                status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
                detail=str(exc),
            ) from exc
        except anakin.AnakinTimeoutError as exc:
            logger.error("Anakin timeout: %s", exc)
            raise HTTPException(
                status_code=status.HTTP_504_GATEWAY_TIMEOUT,
                detail="Anakin search timed out while querying competitor data.",
            ) from exc
        except anakin.AnakinAPIError as exc:
            logger.error("Anakin API error (%d): %s", exc.status_code, exc.detail)
            raise HTTPException(
                status_code=status.HTTP_502_BAD_GATEWAY,
                detail=f"Anakin API returned error status {exc.status_code}",
            ) from exc
        except anakin.AnakinResponseError as exc:
            logger.error("Anakin response error: %s", exc)
            raise HTTPException(
                status_code=status.HTTP_502_BAD_GATEWAY,
                detail="Failed to parse response received from Anakin Search API.",
            ) from exc

    # Step 3: Build combined DronaHQ payload
    dronahq_payload = {
        "message": (
            f"Analyze the following competitor research for {company.name} "
            f"(Products: {', '.join(company.products)}; Segments: {', '.join(company.target_customers)}). "
            "Extract meaningful competitive signals based only on the provided evidence."
        ),
        "company": {
            "name": company.name,
            "products": company.products,
            "target_customers": company.target_customers,
        },
        "competitors": [
            {
                "name": comp_res.competitor,
                "research": [
                    {
                        "title": r.title,
                        "url": r.url,
                        "snippet": r.snippet,
                        "date": r.date,
                        "last_updated": r.last_updated,
                    }
                    for r in comp_res.results
                ],
            }
            for comp_res in all_research
        ],
    }

    # Step 4: Dispatch to DronaHQ reasoning webhook
    logger.info("Triggering DronaHQ reasoning agent webhook...")
    try:
        dronahq_run, signals = await dronahq.trigger_reasoning(
            dronahq_payload,
            research_context=all_research,
        )
    except dronahq.DronaHQConfigurationError as exc:
        logger.error("DronaHQ configuration error: %s", exc)
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=str(exc),
        ) from exc
    except (dronahq.DronaHQTimeoutError, dronahq.DronaHQAPIError, dronahq.DronaHQResponseError) as exc:
        # Fallback resilience: preserve collected Anakin research even if reasoning agent encounters an issue
        logger.warning("DronaHQ reasoning error, preserving collected research: %s", exc)
        dronahq_run = DronaHQRun(
            status="error",
            message=f"Research collected — AI reasoning unavailable: {exc}",
        )
        signals = []

    # Step 5: Construct final response
    return ScanResponse(
        company=company,
        research=all_research,
        reasoning=dronahq_run,
        signals=signals,
    )


@app.post("/api/simulate", response_model=ResponseSimulation, status_code=status.HTTP_200_OK)
async def simulate_signal(request: SimulateRequest):
    """Simulate response options across Product, Sales, Marketing, and Strategy
    for a specific CompetitiveSignal.

    Reuses existing rich signal data and prompts the DronaHQ reasoning agent for
    evidence-grounded decision support options without inventing facts or internal capabilities.
    If DronaHQ is unavailable, provides an evidence-grounded fallback preserving context and source URLs.
    """
    default_company, _ = load_default_company_context()
    company = request.company if request.company else default_company

    logger.info(
        "Initiating response simulation for %s regarding competitor '%s' move '%s'",
        company.name,
        request.signal.competitor,
        request.signal.headline,
    )

    simulation = await dronahq.simulate_response(signal=request.signal, company=company)
    return simulation


@app.post("/api/ask", response_model=AskWarroomResponse, status_code=status.HTTP_200_OK)
async def ask_warroom_endpoint(request: AskWarroomRequest):
    """Answer natural language queries regarding the currently supplied competitive signals.

    Synthesizes answers grounded strictly in the supplied signals and evidence URLs via DronaHQ.
    If no signals are supplied, returns an insufficient-context response without calling external services.
    If DronaHQ is unavailable, provides an evidence-grounded fallback preserving context and source URLs.
    """
    default_company, _ = load_default_company_context()
    company = request.company if request.company else default_company

    logger.info(
        "Received Ask WARROOM query for %s with %d signals: '%s'",
        company.name,
        len(request.signals),
        request.question[:100],
    )

    response = await dronahq.ask_warroom(
        question=request.question,
        signals=request.signals,
        company=company,
    )
    return response


# ==============================================================================
# A2A / Nasiko Protocol Endpoints
# ==============================================================================


@app.get("/.well-known/agent-card.json", status_code=status.HTTP_200_OK)
@app.get("/.well-known/agent.json", status_code=status.HTTP_200_OK)
async def get_agent_card_endpoint():
    """A2A Agent Card discovery endpoint (spec v1.0 standard and legacy v0.3)."""
    return nasiko.get_agent_card()



@app.post("/a2a", status_code=status.HTTP_200_OK)
async def a2a_rpc_endpoint(request: Request):
    """A2A Protocol JSON-RPC 2.0 dispatch endpoint."""
    try:
        payload = await request.json()
    except Exception:
        return JSONResponse(
            status_code=status.HTTP_200_OK,
            content=nasiko.jsonrpc_error(None, -32600, "Invalid JSON in request body"),
        )

    result = await nasiko.dispatch_a2a_request(payload, scan_func=scan_competitors)
    return JSONResponse(status_code=status.HTTP_200_OK, content=result)


