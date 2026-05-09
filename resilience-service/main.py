from fastapi import FastAPI, Request, HTTPException
from fastapi.responses import HTMLResponse, StreamingResponse
from fastapi.middleware.cors import CORSMiddleware
import json
import asyncio
import time

from opentelemetry import trace
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.instrumentation.fastapi import FastAPIInstrumentor
from sse_starlette.sse import EventSourceResponse

from cache_manager import SemanticCache
from rate_limiter import AdaptiveRateLimiter, Priority
from circuit_breaker import PredictiveCircuitBreaker
from metrics import MetricsCollector

app = FastAPI(
    title="Nasiko Resilience Layer",
    description="Intelligent request management for multi-agent systems",
    version="1.0.0"
)

# Configure tracing for Phoenix (localhost:6006)
trace.set_tracer_provider(TracerProvider())
tracer = trace.get_tracer(__name__)

# Phoenix OTLP endpoint
otlp_exporter = OTLPSpanExporter(endpoint="http://phoenix:6006/v1/traces")
span_processor = BatchSpanProcessor(otlp_exporter)
trace.get_tracer_provider().add_span_processor(span_processor)

# Instrument FastAPI
FastAPIInstrumentor.instrument_app(app)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)

# Initialize components
cache = SemanticCache()
rate_limiter = AdaptiveRateLimiter()
circuit_breaker = PredictiveCircuitBreaker()
metrics = MetricsCollector()

# ───────────────────────────────────────────────
# HEALTH & METRICS ENDPOINTS
# ───────────────────────────────────────────────

@app.get("/resilience/health")
async def health():
    return {
        "status": "healthy",
        "service": "resilience-layer",
        "version": "1.0.0",
        "components": {
            "cache": "connected",
            "rate_limiter": "connected",
            "circuit_breaker": "connected"
        }
    }

@app.get("/resilience/metrics")
async def get_metrics():
    return metrics.get_all_metrics()

@app.get("/resilience/alerts")
async def get_alerts():
    return {
        "timestamp": time.time(),
        "alerts": metrics.get_predictive_alerts()
    }

@app.get("/resilience/metrics/dashboard", response_class=HTMLResponse)
async def dashboard():
    html_content = """
    <!DOCTYPE html>
    <html>
    <head>
        <title>Nasiko Resilience Dashboard</title>
        <meta http-equiv="refresh" content="2">
        <style>
            body { font-family: 'Segoe UI', sans-serif; background: #0f172a; color: #e2e8f0; margin: 0; padding: 20px; }
            .header { text-align: center; margin-bottom: 30px; }
            .header h1 { color: #38bdf8; margin: 0; }
            .header p { color: #94a3b8; margin: 5px 0; }
            .grid { display: grid; grid-template-columns: repeat(auto-fit, minmax(300px, 1fr)); gap: 20px; max-width: 1400px; margin: 0 auto; }
            .card { background: #1e293b; border-radius: 12px; padding: 20px; border: 1px solid #334155; }
            .card h3 { margin-top: 0; color: #38bdf8; font-size: 14px; text-transform: uppercase; letter-spacing: 1px; }
            .metric { display: flex; justify-content: space-between; margin: 10px 0; padding: 8px 0; border-bottom: 1px solid #334155; }
            .metric:last-child { border-bottom: none; }
            .value { font-weight: bold; color: #22d3ee; }
            .status-healthy { color: #4ade80; }
            .status-warning { color: #fbbf24; }
            .status-danger { color: #f87171; }
            .badge { padding: 4px 12px; border-radius: 20px; font-size: 12px; font-weight: bold; }
            .badge-green { background: #166534; color: #4ade80; }
            .badge-yellow { background: #713f12; color: #fbbf24; }
            .badge-red { background: #7f1d1d; color: #f87171; }
            .cost-saved { font-size: 24px; color: #4ade80; text-align: center; margin: 10px 0; }
            .refresh-time { text-align: center; color: #64748b; font-size: 12px; margin-top: 20px; }
        </style>
    </head>
    <body>
        <div class="header">
            <h1>Nasiko Resilience Layer</h1>
            <p>Real-time Operational Dashboard</p>
        </div>
        <div class="grid" id="dashboard">
            <div class="card">
                <h3>Overall Health</h3>
                <div class="metric">
                    <span>Service Status</span>
                    <span class="badge badge-green">HEALTHY</span>
                </div>
                <div class="metric">
                    <span>Active Agents</span>
                    <span class="value" id="total-agents">Loading...</span>
                </div>
                <div class="metric">
                    <span>Open Circuits</span>
                    <span class="value status-danger" id="open-circuits">Loading...</span>
                </div>
            </div>
            <div class="card">
                <h3>Cost Savings</h3>
                <div class="cost-saved" id="cost-saved">$0.00</div>
                <div class="metric">
                    <span>Total Cache Hits</span>
                    <span class="value" id="cache-hits">0</span>
                </div>
                <div class="metric">
                    <span>Cache Hit Rate</span>
                    <span class="value" id="hit-rate">0%</span>
                </div>
            </div>
            <div class="card">
                <h3>Rate Limiting</h3>
                <div class="metric">
                    <span>Total Queued</span>
                    <span class="value" id="total-queued">0</span>
                </div>
                <div class="metric">
                    <span>P0 Queue</span>
                    <span class="value status-healthy" id="queue-p0">0</span>
                </div>
                <div class="metric">
                    <span>P1 Queue</span>
                    <span class="value status-warning" id="queue-p1">0</span>
                </div>
                <div class="metric">
                    <span>P2 Queue</span>
                    <span class="value status-danger" id="queue-p2">0</span>
                </div>
            </div>
            <div class="card">
                <h3>Circuit Breakers</h3>
                <div id="circuit-list">Loading...</div>
            </div>
        </div>
        <div class="refresh-time">Auto-refreshing every 2 seconds</div>
        <script>
            async function fetchMetrics() {
                try {
                    const res = await fetch('/resilience/metrics');
                    const data = await res.json();
                    
                    document.getElementById('total-agents').textContent = data.summary.total_agents;
                    document.getElementById('open-circuits').textContent = data.summary.open_circuits;
                    document.getElementById('cost-saved').textContent = '$' + data.summary.total_cost_saved_usd.toFixed(4);
                    document.getElementById('cache-hits').textContent = data.summary.total_cache_hits;
                    
                    let totalHits = 0, totalReqs = 0;
                    let circuitHtml = '';
                    
                    for (const [agent, m] of Object.entries(data.agents)) {
                        totalHits += m.cache.active_entries;
                        totalReqs += m.circuit.total_requests;
                        
                        const stateClass = m.circuit.state === 'OPEN' ? 'badge-red' : 
                                          m.circuit.state === 'HALF_OPEN' ? 'badge-yellow' : 'badge-green';
                        
                        circuitHtml += `
                            <div class="metric">
                                <span>${agent}</span>
                                <span class="badge ${stateClass}">${m.circuit.state}</span>
                            </div>
                            <div style="font-size: 11px; color: #64748b; margin-bottom: 8px;">
                                Error: ${(m.circuit.error_rate * 100).toFixed(1)}% | Cache: ${(m.cache.hit_rate * 100).toFixed(1)}%
                            </div>
                        `;
                    }
                    
                    document.getElementById('hit-rate').textContent = totalReqs > 0 ? 
                        ((totalHits / totalReqs) * 100).toFixed(1) + '%' : '0%';
                    document.getElementById('total-queued').textContent = data.summary.total_queued_requests;
                    
                    // Queue depths from first agent (simplified)
                    const firstAgent = Object.values(data.agents)[0];
                    if (firstAgent) {
                        document.getElementById('queue-p0').textContent = firstAgent.rate_limit.queue_depths.P0;
                        document.getElementById('queue-p1').textContent = firstAgent.rate_limit.queue_depths.P1;
                        document.getElementById('queue-p2').textContent = firstAgent.rate_limit.queue_depths.P2;
                    }
                    
                    document.getElementById('circuit-list').innerHTML = circuitHtml || '<div style="color: #64748b;">No active agents</div>';
                    
                } catch (e) {
                    console.error('Failed to fetch metrics:', e);
                }
            }
            fetchMetrics();
            setInterval(fetchMetrics, 2000);
        </script>
    </body>
    </html>
    """
    return html_content

# ───────────────────────────────────────────────
# CACHE ENDPOINTS
# ───────────────────────────────────────────────

@app.post("/resilience/cache/check")
async def check_cache(request: Request):
    body = await request.json()
    query = body.get("query")
    agent = body.get("agent", "translator")
    
    if not query:
        raise HTTPException(status_code=400, detail="query is required")
    
    result = cache.check_cache(query, agent)
    
    if result["cache_hit"]:
        metrics.record_cache_hit(agent)
    
    return result

@app.post("/resilience/cache/store")
async def store_cache(request: Request):
    body = await request.json()
    query = body.get("query")
    response = body.get("response")
    agent = body.get("agent", "translator")
    
    if not query or response is None:
        raise HTTPException(status_code=400, detail="query and response are required")
    
    return cache.store_cache(query, response, agent)

@app.get("/resilience/cache/stats/{agent}")
async def cache_stats(agent: str):
    return cache.get_cache_stats(agent)

@app.delete("/resilience/cache/{agent}")
async def invalidate_cache(agent: str):
    return cache.invalidate_agent_cache(agent)

# ───────────────────────────────────────────────
# RATE LIMIT ENDPOINTS
# ───────────────────────────────────────────────

@app.post("/resilience/ratelimit/check")
async def check_rate_limit(request: Request):
    body = await request.json()
    agent = body.get("agent", "translator")
    priority = body.get("priority", "P1")
    request_id = body.get("request_id")
    
    return rate_limiter.check_rate_limit(agent, priority, request_id)

@app.post("/resilience/ratelimit/refill/{agent}")
async def refill_tokens(agent: str, amount: int = 10):
    return rate_limiter.refill_tokens(agent, amount)

@app.get("/resilience/ratelimit/queue/{agent}")
async def queue_status(agent: str):
    return rate_limiter.get_queue_status(agent)

@app.post("/resilience/config/ratelimit/{agent}")
async def update_rate_limit(agent: str, request: Request):
    body = await request.json()
    tokens_per_minute = body.get("tokens_per_minute", 100)
    return rate_limiter.update_rate_limit(agent, tokens_per_minute)

# ───────────────────────────────────────────────
# CIRCUIT BREAKER ENDPOINTS
# ───────────────────────────────────────────────

@app.get("/resilience/circuit/{agent}")
async def circuit_status(agent: str):
    return circuit_breaker.get_status(agent)

@app.post("/resilience/circuit/{agent}/record")
async def record_circuit(agent: str, request: Request):
    body = await request.json()
    success = body.get("success", True)
    return circuit_breaker.record_request(agent, success)

@app.post("/resilience/circuit/{agent}/reset")
async def reset_circuit(agent: str):
    return circuit_breaker.manual_reset(agent)

# ───────────────────────────────────────────────
# CONFIG ENDPOINTS
# ───────────────────────────────────────────────

@app.post("/resilience/config/cache")
async def update_cache_config(request: Request):
    body = await request.json()
    # Hot reload cache settings
    if "similarity_threshold" in body:
        cache.similarity_threshold = body["similarity_threshold"]
    if "ttl" in body:
        cache.default_ttl = body["ttl"]
    
    return {
        "updated": True,
        "similarity_threshold": cache.similarity_threshold,
        "ttl": cache.default_ttl
    }

# ───────────────────────────────────────────────
# BACKGROUND TASKS
# ───────────────────────────────────────────────

@app.on_event("startup")
async def startup_event():
    async def token_refill_loop():
        while True:
            try:
                # Refill tokens for all known agents every 10 seconds
                agents = ["translator"]  # Extend with discovery
                for agent in agents:
                    rate_limiter.refill_tokens(agent)
            except Exception:
                pass
            await asyncio.sleep(10)
    
    asyncio.create_task(token_refill_loop())


@app.get("/resilience/health/detailed")
async def detailed_health():
    """Detailed health check with dependency status"""
    redis_status = "healthy"
    try:
        cache.redis.ping()
    except Exception:
        redis_status = "degraded"
    
    return {
        "status": "healthy" if redis_status == "healthy" else "degraded",
        "service": "resilience-layer",
        "dependencies": {
            "redis": redis_status,
            "cache": "connected",
            "rate_limiter": "connected",
            "circuit_breaker": "connected"
        },
        "mode": "normal" if redis_status == "healthy" else "pass-through"
    }

if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=8090)
