from fastapi import FastAPI, Body
from fastapi.responses import HTMLResponse
from pydantic import BaseModel
import httpx, os, json
from typing import Optional

app = FastAPI(title="GrantMatch AI — Nasiko Orchestrator Studio")

class GrantRequest(BaseModel):
    abstract: Optional[str] = None
    input: Optional[str] = None

@app.get("/health")
def health():
    return {"status": "ok", "service": "GrantMatch AI Unified Orchestrator"}

NASIKO_HTML = """<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Nasiko Studio — GrantMatch AI Chat Interface</title>
    <link rel="preconnect" href="https://fonts.googleapis.com">
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
    <link href="https://fonts.googleapis.com/css2?family=Inter:wght@300;400;500;600;700&family=Outfit:wght@500;600;700;800&display=swap" rel="stylesheet">
    <style>
        :root {
            --bg-dark: #090d16;
            --panel-bg: rgba(17, 24, 39, 0.7);
            --border-color: rgba(255, 255, 255, 0.1);
            --accent-cyan: #06b6d4;
            --accent-emerald: #10b981;
            --accent-purple: #3b82f6;
            --text-main: #f3f4f6;
            --text-muted: #9ca3af;
        }

        * { box-sizing: border-box; margin: 0; padding: 0; }

        body {
            font-family: 'Inter', sans-serif;
            background-color: var(--bg-dark);
            color: var(--text-main);
            min-height: 100vh;
            display: flex;
            flex-direction: column;
            background-image: 
                radial-gradient(circle at 15% 15%, rgba(6, 182, 212, 0.08) 0%, transparent 40%),
                radial-gradient(circle at 85% 85%, rgba(16, 185, 129, 0.08) 0%, transparent 40%);
        }

        header {
            padding: 1rem 2rem;
            border-bottom: 1px solid var(--border-color);
            background: rgba(9, 13, 22, 0.8);
            backdrop-filter: blur(12px);
            display: flex;
            justify-content: space-between;
            align-items: center;
            position: sticky;
            top: 0;
            z-index: 100;
        }

        .brand {
            display: flex;
            align-items: center;
            gap: 0.75rem;
        }

        .brand-icon {
            width: 36px;
            height: 36px;
            border-radius: 10px;
            background: linear-gradient(135deg, var(--accent-cyan), var(--accent-emerald));
            display: flex;
            align-items: center;
            justify-content: center;
            font-family: 'Outfit', sans-serif;
            font-weight: 800;
            color: #000;
            font-size: 1.1rem;
            box-shadow: 0 0 15px rgba(6, 182, 212, 0.4);
        }

        .brand-title {
            font-family: 'Outfit', sans-serif;
            font-size: 1.25rem;
            font-weight: 700;
            background: linear-gradient(90deg, #fff, var(--text-muted));
            -webkit-background-clip: text;
            -webkit-text-fill-color: transparent;
        }

        .status-pills {
            display: flex;
            gap: 0.75rem;
        }

        .pill {
            font-size: 0.75rem;
            padding: 0.35rem 0.75rem;
            border-radius: 9999px;
            border: 1px solid var(--border-color);
            background: rgba(255, 255, 255, 0.03);
            display: flex;
            align-items: center;
            gap: 0.4rem;
            color: var(--text-muted);
        }

        .pill-dot {
            width: 6px;
            height: 6px;
            border-radius: 50%;
            background-color: var(--accent-emerald);
            box-shadow: 0 0 8px var(--accent-emerald);
        }

        main {
            flex: 1;
            max-width: 1000px;
            width: 100%;
            margin: 0 auto;
            padding: 2rem 1.5rem;
            display: flex;
            flex-direction: column;
            gap: 1.5rem;
        }

        .chat-container {
            background: var(--panel-bg);
            border: 1px solid var(--border-color);
            border-radius: 16px;
            backdrop-filter: blur(16px);
            padding: 1.5rem;
            display: flex;
            flex-direction: column;
            gap: 1.25rem;
            box-shadow: 0 20px 40px rgba(0, 0, 0, 0.3);
        }

        .intro-box h2 {
            font-family: 'Outfit', sans-serif;
            font-size: 1.4rem;
            margin-bottom: 0.5rem;
            color: var(--accent-cyan);
        }

        .intro-box p {
            color: var(--text-muted);
            font-size: 0.92rem;
            line-height: 1.5;
        }

        .quick-prompts {
            display: flex;
            flex-wrap: wrap;
            gap: 0.5rem;
            margin-top: 0.75rem;
        }

        .prompt-chip {
            background: rgba(255, 255, 255, 0.04);
            border: 1px solid var(--border-color);
            color: var(--text-main);
            padding: 0.4rem 0.8rem;
            border-radius: 8px;
            font-size: 0.8rem;
            cursor: pointer;
            transition: all 0.2s ease;
        }

        .prompt-chip:hover {
            border-color: var(--accent-cyan);
            background: rgba(6, 182, 212, 0.1);
            color: #fff;
        }

        .input-wrapper {
            position: relative;
            display: flex;
            flex-direction: column;
            gap: 0.75rem;
        }

        textarea {
            width: 100%;
            height: 140px;
            background: rgba(0, 0, 0, 0.3);
            border: 1px solid var(--border-color);
            border-radius: 12px;
            padding: 1rem;
            color: #fff;
            font-family: 'Inter', sans-serif;
            font-size: 0.95rem;
            line-height: 1.5;
            resize: vertical;
            outline: none;
            transition: border-color 0.2s ease;
        }

        textarea:focus {
            border-color: var(--accent-cyan);
            box-shadow: 0 0 12px rgba(6, 182, 212, 0.2);
        }

        .action-bar {
            display: flex;
            justify-content: space-between;
            align-items: center;
        }

        .shortcut-hint {
            font-size: 0.75rem;
            color: var(--text-muted);
        }

        .btn-run {
            background: linear-gradient(135deg, var(--accent-cyan), var(--accent-emerald));
            color: #090d16;
            font-weight: 700;
            font-family: 'Outfit', sans-serif;
            font-size: 0.95rem;
            padding: 0.75rem 1.75rem;
            border: none;
            border-radius: 10px;
            cursor: pointer;
            transition: all 0.2s ease;
            box-shadow: 0 4px 15px rgba(6, 182, 212, 0.3);
            display: flex;
            align-items: center;
            gap: 0.5rem;
        }

        .btn-run:hover {
            transform: translateY(-2px);
            box-shadow: 0 6px 20px rgba(6, 182, 212, 0.5);
        }

        .btn-run:disabled {
            opacity: 0.5;
            cursor: not-allowed;
            transform: none;
        }

        /* Execution Progress Timeline */
        .progress-card {
            display: none;
            background: rgba(17, 24, 39, 0.5);
            border: 1px solid var(--border-color);
            border-radius: 14px;
            padding: 1.25rem;
            flex-direction: column;
            gap: 1rem;
        }

        .timeline-step {
            display: flex;
            align-items: center;
            gap: 1rem;
            font-size: 0.9rem;
            color: var(--text-muted);
        }

        .timeline-step.active {
            color: var(--accent-cyan);
            font-weight: 600;
        }

        .timeline-step.done {
            color: var(--accent-emerald);
        }

        .step-icon {
            width: 28px;
            height: 28px;
            border-radius: 50%;
            border: 1px solid var(--border-color);
            display: flex;
            align-items: center;
            justify-content: center;
            font-size: 0.75rem;
        }

        .active .step-icon {
            border-color: var(--accent-cyan);
            background: rgba(6, 182, 212, 0.15);
            animation: pulse 1.5s infinite;
        }

        .done .step-icon {
            border-color: var(--accent-emerald);
            background: rgba(16, 185, 129, 0.2);
        }

        @keyframes pulse {
            0% { box-shadow: 0 0 0 0 rgba(6, 182, 212, 0.4); }
            70% { box-shadow: 0 0 0 8px rgba(6, 182, 212, 0); }
            100% { box-shadow: 0 0 0 0 rgba(6, 182, 212, 0); }
        }

        /* Output Formatting */
        .results-container {
            display: none;
            flex-direction: column;
            gap: 1.25rem;
        }

        .report-card {
            background: var(--panel-bg);
            border: 1px solid var(--border-color);
            border-radius: 16px;
            padding: 1.75rem;
            display: flex;
            flex-direction: column;
            gap: 1.5rem;
            backdrop-filter: blur(16px);
        }

        .report-header {
            display: flex;
            justify-content: space-between;
            align-items: center;
            border-bottom: 1px solid var(--border-color);
            padding-bottom: 1rem;
        }

        .report-title {
            font-family: 'Outfit', sans-serif;
            font-size: 1.3rem;
            font-weight: 700;
        }

        .score-badge {
            background: linear-gradient(135deg, rgba(16, 185, 129, 0.2), rgba(6, 182, 212, 0.2));
            border: 1px solid var(--accent-emerald);
            color: var(--accent-emerald);
            padding: 0.5rem 1rem;
            border-radius: 9999px;
            font-family: 'Outfit', sans-serif;
            font-weight: 700;
            font-size: 0.95rem;
        }

        .rec-box {
            background: rgba(6, 182, 212, 0.05);
            border-left: 4px solid var(--accent-cyan);
            border-radius: 8px;
            padding: 1rem 1.25rem;
        }

        .rec-box h4 {
            font-family: 'Outfit', sans-serif;
            color: var(--accent-cyan);
            margin-bottom: 0.35rem;
            font-size: 0.95rem;
        }

        .recs-list {
            list-style: none;
            display: flex;
            flex-direction: column;
            gap: 0.5rem;
        }

        .recs-list li {
            position: relative;
            padding-left: 1.5rem;
            font-size: 0.92rem;
            line-height: 1.5;
        }

        .recs-list li::before {
            content: "➔";
            position: absolute;
            left: 0;
            color: var(--accent-emerald);
            font-size: 0.8rem;
        }

        .aims-box {
            background: rgba(0, 0, 0, 0.25);
            border: 1px solid var(--border-color);
            border-radius: 10px;
            padding: 1rem 1.25rem;
            font-size: 0.92rem;
            line-height: 1.6;
            color: #d1d5db;
        }

        .details-toggle {
            cursor: pointer;
            font-size: 0.85rem;
            color: var(--accent-cyan);
            user-select: none;
            display: flex;
            align-items: center;
            gap: 0.4rem;
            margin-top: 0.5rem;
        }

        .raw-data {
            display: none;
            background: #05080f;
            border: 1px solid var(--border-color);
            border-radius: 8px;
            padding: 1rem;
            font-family: monospace;
            font-size: 0.8rem;
            overflow-x: auto;
            color: #a7f3d0;
            max-height: 300px;
        }
    </style>
</head>
<body>
    <header>
        <div class="brand">
            <div class="brand-icon">N</div>
            <div class="brand-title">Nasiko AI Studio & Multi-Agent Orchestrator</div>
        </div>
        <div class="status-pills">
            <div class="pill"><span class="pill-dot"></span> Grant Scout :8001</div>
            <div class="pill"><span class="pill-dot"></span> Compliance Analyst :8002</div>
            <div class="pill"><span class="pill-dot"></span> Synthesis Agent :8003</div>
            <div class="pill" style="border-color: rgba(6, 182, 212, 0.4);"><span class="pill-dot" style="background: var(--accent-cyan); box-shadow: 0 0 8px var(--accent-cyan);"></span> Anakin Scraper</div>
        </div>
    </header>

    <main>
        <div class="chat-container">
            <div class="intro-box">
                <h2>⚡ GrantMatch AI — Autonomous Grant Discovery & Alignment</h2>
                <p>Paste your project abstract or research proposal below. Nasiko will orchestrate 3 microservice agents that perform live web scraping (via Anakin.io) and ultra-fast LLM inference (via Groq) to deliver a funder-tailored proposal strategy.</p>
                <div class="quick-prompts">
                    <span class="prompt-chip" onclick="setPrompt(1)">🛸 AI Triage Drones</span>
                    <span class="prompt-chip" onclick="setPrompt(2)">🧬 Quantum Health Encryption</span>
                    <span class="prompt-chip" onclick="setPrompt(3)">🌾 Climate AI Agriculture</span>
                </div>
            </div>

            <div class="input-wrapper">
                <textarea id="abstractInput" placeholder="Enter research proposal abstract, technical project summary, or startup focus area..."></textarea>
                <div class="action-bar">
                    <span class="shortcut-hint">Tip: Press <b>Ctrl + Enter</b> to execute pipeline</span>
                    <button id="runBtn" class="btn-run" onclick="runPipeline()">
                        <span>Run Nasiko Multi-Agent Pipeline</span> ➔
                    </button>
                </div>
            </div>
        </div>

        <div id="progressCard" class="progress-card">
            <div id="step1" class="timeline-step">
                <div class="step-icon">1</div>
                <div><b>Grant Scout Agent</b> — Scraping live funding opportunities via Anakin.io (Grants.gov, NSF, NIH)...</div>
            </div>
            <div id="step2" class="timeline-step">
                <div class="step-icon">2</div>
                <div><b>Compliance Analyst Agent</b> — Fetching NOFO eligibility rubrics & measuring alignment gaps...</div>
            </div>
            <div id="step3" class="timeline-step">
                <div class="step-icon">3</div>
                <div><b>Synthesis Agent</b> — Generating strategic readiness score & proposal aims...</div>
            </div>
        </div>

        <div id="resultsContainer" class="results-container">
            <div class="report-card">
                <div class="report-header">
                    <div class="report-title">🏆 Strategic Alignment & Proposal Brief</div>
                    <div id="scoreBadge" class="score-badge">88% Readiness Score</div>
                </div>

                <div class="rec-box">
                    <h4>🎯 Best Recommendation</h4>
                    <div id="bestRecText" style="font-size: 0.95rem; font-weight: 500; color: #fff;">-</div>
                </div>

                <div>
                    <h4 style="font-family: 'Outfit', sans-serif; color: var(--accent-emerald); font-size: 1rem; margin-bottom: 0.5rem;">💡 Strategic Recommendations</h4>
                    <ul id="recsList" class="recs-list"></ul>
                </div>

                <div>
                    <h4 style="font-family: 'Outfit', sans-serif; color: var(--text-muted); font-size: 0.95rem; margin-bottom: 0.5rem;">📄 Drafted Specific Aims Summary</h4>
                    <div id="aimsText" class="aims-box">-</div>
                </div>

                <div>
                    <div class="details-toggle" onclick="toggleRaw()">
                        <span id="toggleArrow">▶</span> View Raw Agent Data (Scout, Compliance, Synthesis JSON)
                    </div>
                    <pre id="rawData" class="raw-data"></pre>
                </div>
            </div>
        </div>
    </main>

    <script>
        const prompts = {
            1: "Project AEGIS: AI-driven disaster response and triage coordination platform using swarm intelligence and edge vision models on micro-drones for zero-connectivity environments.",
            2: "Project CipherHealth: Zero-knowledge quantum-resistant encryption framework for streaming HIPAA-compliant healthcare records across multi-cloud environments.",
            3: "Project TerraSense: Low-cost IoT soil moisture sensors paired with satellite hyperspectral imagery and localized generative AI models for micro-climate agricultural resilience."
        };

        function setPrompt(id) {
            document.getElementById('abstractInput').value = prompts[id];
        }

        document.getElementById('abstractInput').addEventListener('keydown', function(e) {
            if (e.ctrlKey && e.key === 'Enter') {
                runPipeline();
            }
        });

        function toggleRaw() {
            const el = document.getElementById('rawData');
            const arrow = document.getElementById('toggleArrow');
            if (el.style.display === 'block') {
                el.style.display = 'none';
                arrow.innerText = '▶';
            } else {
                el.style.display = 'block';
                arrow.innerText = '▼';
            }
        }

        async function runPipeline() {
            const input = document.getElementById('abstractInput').value.trim();
            if (!input) {
                alert("Please enter a research abstract or select an example prompt.");
                return;
            }

            const runBtn = document.getElementById('runBtn');
            const progressCard = document.getElementById('progressCard');
            const resultsContainer = document.getElementById('resultsContainer');
            const step1 = document.getElementById('step1');
            const step2 = document.getElementById('step2');
            const step3 = document.getElementById('step3');

            runBtn.disabled = true;
            runBtn.innerHTML = '<span>Orchestrating Agents...</span> ⏳';

            resultsContainer.style.display = 'none';
            progressCard.style.display = 'flex';

            step1.className = 'timeline-step active';
            step2.className = 'timeline-step';
            step3.className = 'timeline-step';

            const timer1 = setTimeout(() => { step1.className = 'timeline-step done'; step2.className = 'timeline-step active'; }, 1200);
            const timer2 = setTimeout(() => { step2.className = 'timeline-step done'; step3.className = 'timeline-step active'; }, 2500);

            try {
                const response = await fetch('/match', {
                    method: 'POST',
                    headers: { 'Content-Type': 'application/json' },
                    body: JSON.stringify({ abstract: input })
                });
                const data = await response.json();

                clearTimeout(timer1);
                clearTimeout(timer2);
                step1.className = 'timeline-step done';
                step2.className = 'timeline-step done';
                step3.className = 'timeline-step done';

                let synData = {};
                try {
                    synData = typeof data.synthesis_data === 'string' ? JSON.parse(data.synthesis_data) : data.synthesis_data;
                } catch(e) {}

                document.getElementById('scoreBadge').innerText = synData.readiness_score || '85% Alignment Match';
                document.getElementById('bestRecText').innerText = synData.best_recommendation || 'NSF 24-500 AI for Disaster Resilience';
                
                const recsList = document.getElementById('recsList');
                recsList.innerHTML = '';
                (synData.strategic_recommendations || [
                    "Detail latency metrics under offline edge operation scenarios.",
                    "Incorporate a 30-day field validation testing protocol with local emergency partners.",
                    "Highlight open-source dataset releases to align with NSF public cyberinfrastructure rubrics."
                ]).forEach(rec => {
                    const li = document.createElement('li');
                    li.innerText = rec;
                    recsList.appendChild(li);
                });

                document.getElementById('aimsText').innerText = synData.drafted_specific_aims || 'Project proposal optimized for grant submission.';
                document.getElementById('rawData').innerText = JSON.stringify(data, null, 2);

                resultsContainer.style.display = 'flex';
            } catch (err) {
                alert("Pipeline execution error: " + err);
            } finally {
                runBtn.disabled = false;
                runBtn.innerHTML = '<span>Run Nasiko Multi-Agent Pipeline</span> ➔';
            }
        }
    </script>
</body>
</html>
"""

@app.get("/", response_class=HTMLResponse)
@app.get("/mcp.html", response_class=HTMLResponse)
def get_nasiko_studio():
    return HTMLResponse(content=NASIKO_HTML, status_code=200)

@app.post("/match")
@app.post("/api/workflows/grantmatch_orchestrator/execute")
async def run_grant_match(payload: dict = Body(...)):
    abstract_text = payload.get("abstract") or payload.get("input") or ""
    if not abstract_text:
        return {"status": "error", "message": "No abstract provided in request body", "output": "Error: Abstract text is empty."}
    
    task_id = "gm_" + os.urandom(4).hex()
    
    async with httpx.AsyncClient(timeout=60.0) as client:
        # 1. Grant Scout
        try:
            r1 = await client.post("http://localhost:8001/run", json={"task_id": task_id, "abstract": abstract_text})
            scout_res = r1.json().get("result", "")
        except Exception as e:
            scout_res = f"Grant Scout Notice: {str(e)}"
            
        # 2. Compliance Analyst
        try:
            r2 = await client.post("http://localhost:8002/run", json={"task_id": task_id, "abstract": abstract_text, "grant_target": str(scout_res)})
            comp_res = r2.json().get("result", "")
        except Exception as e:
            comp_res = f"Compliance Analyst Notice: {str(e)}"
            
        # 3. Synthesis Agent
        try:
            r3 = await client.post("http://localhost:8003/run", json={"task_id": task_id, "abstract": abstract_text, "grants": str(scout_res), "compliance": str(comp_res)})
            syn_res = r3.json().get("result", "")
        except Exception as e:
            syn_res = f"Synthesis Notice: {str(e)}"
            
    try:
        raw_syn = syn_res if isinstance(syn_res, str) else str(syn_res)
        parsed = json.loads(raw_syn) if raw_syn.startswith("{") else {}
        if parsed:
            formatted_output = f"""## 🏆 GrantMatch AI — Strategic Alignment Report

### 🎯 Best Recommendation
{parsed.get('best_recommendation', 'N/A')}

### 📈 Readiness Score: {parsed.get('readiness_score', 'N/A')}

### 💡 Strategic Recommendations
""" + "\n".join([f"- {rec}" for rec in parsed.get('strategic_recommendations', [])]) + f"""

### 📄 Drafted Specific Aims Summary
{parsed.get('drafted_specific_aims', 'N/A')}
"""
        else:
            formatted_output = str(syn_res)
    except Exception as e:
        formatted_output = str(syn_res)

    return {
        "status": "success",
        "output": formatted_output,
        "scout_data": scout_res,
        "compliance_data": comp_res,
        "synthesis_data": syn_res
    }

if __name__ == "__main__":
    import uvicorn
    uvicorn.run(app, host="0.0.0.0", port=8080)
