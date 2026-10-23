# 🏆 Buildathon Plan — GrantMatch AI: Intelligent Grant Discovery & Proposal Alignment Platform

## The Big Idea

**GrantMatch AI** is an intelligent grant-writing and alignment assistant designed to solve one of the most tedious administrative bottlenecks for researchers, startups, and NGOs: finding the right funding and tailoring proposals to match exact rubrics.

A user uploads their project abstract or research proposal into a **DronaHQ** dashboard $\rightarrow$ **Nasiko** orchestrates 4 specialist agents that use **Anakin.io** to scrape live grant portals (e.g., NSF, NIH, global foundations) $\rightarrow$ the system cross-references requirements, flags compliance gaps, and auto-generates a tailored alignment brief using **Groq**'s ultra-fast inference.

> **The pitch:** *"Securing research or startup funding usually takes months of hunting guidelines and rewriting drafts. GrantMatch AI matches your project to live grants and builds a compliance-optimized alignment brief in under 60 seconds."*

---

## Tool Roles

| Tool | Role | What it does |
| --- | --- | --- |
| 🧠 **Nasiko** | **Orchestrator** | Routes tasks between specialist agents via A2A protocol. Provides a live visual graph demo wow-factor.

 |
| 👁️ **Anakin.io** | **Grant Scanner** | Scrapes live grant listings, NOFOs (Notice of Funding Opportunities), and foundation criteria as clean markdown.

 |
| 🖥️ **DronaHQ** | **User Dashboard** | No-code UI: project input $\rightarrow$ agent status tracker $\rightarrow$ structured match score & alignment report.

 |
| ⚡ **Groq** | **LLM Engine** | Ultra-fast inference (500+ tok/s) — generates the compliance analysis and section drafts in seconds.

 |

---

## Architecture

```
┌───────────────────────────────────────────────┐
│              DronaHQ Dashboard                │
│  Project Abstract: "Quantum Cryptography..." │
│  [Find Grants] → [Status] → [Match & Brief]   │
└──────────────────────┬────────────────────────┘
                       │ POST /api/v1/tasks
                       ▼
┌───────────────────────────────────────────────┐
│          Nasiko (A2A Orchestrator)            │
│  Routes tasks → monitors multi-agent flow     │
└──────┬───────────────┬────────────────┬───────┘
       │               │                │
       ▼               ▼                ▼
┌──────────────┐┌──────────────┐┌──────────────┐
│  Grant Scout ││ Compliance   ││ Budget & Fit │
│    Agent     ││   Analyst    ││   Auditor    │
└──────┬───────┘└──────┬───────┘└──────┬───────┘
       │               │               │
       └───────────────┴───────────────┘
                       │ (all use Anakin.io)
                       ▼
             ┌──────────────────┐
             │  Anakin.io API   │
             │ Grants.gov • NSF │
             │ NIH • Foundations│
             └──────────────────┘
                       │
                       ▼
             ┌──────────────────┐
             │ Synthesis Agent  │
             │ (Groq: 70B model)│
             └──────────────────┘
                       │
                       ▼
             Structured Match & Alignment → DronaHQ

```

---

## The 4 Agents

| Agent | Anakin.io call | Groq task | Output |
| --- | --- | --- | --- |
| **Grant Scout** | Search live databases for active grants matching keywords | Extract top funding opportunities: name, agency, deadline, max award | `{grants: [{title, agency, deadline, max_amount, url}]}` |
| **Compliance Analyst** | Scrape specific NOFO/RFP requirements and review criteria | Map project goals against funder priorities and identify mismatches | `{compliance_score: "85%", alignment_gaps: [...]}` |
| **Budget & Fit Auditor** | Scrape cost principles and eligibility guidelines | Check if project scale fits the funding tier and structural limits | `{eligibility_status: "Eligible", recommendations: [...]}` |
| **Synthesis Agent** | — | Merge findings into a unified grant match and strategy report | `{summary, top_match, alignment_strategy, drafted_specific_aims}` |

---

## 24-Hour Timeline

### ⏱ Hour 0–2: Environment Setup

* [ ] Sign up at [console.groq.com](https://console.groq.com?utm_source=gemini) $\rightarrow$ get free API key


* [ ] Sign up at [anakin.io](https://anakin.io?utm_source=gemini) $\rightarrow$ get free API key (300 credits, no card)


* [ ] Add keys to `server/.env` and `agents/.env`:
```
GROQ_API_KEY=gsk_...
ANAKIN_API_KEY=...

```


* [ ] Create workspace directory `c:\nasiko\grant_agents\`

### ⏱ Hour 2–8: Build the 4 Agents

**Grant Scout Agent (~80 lines):**

```python
# agents/grant_scout/main.py
from fastapi import FastAPI
from pydantic import BaseModel
import httpx, os
from groq import Groq

app = FastAPI()
llm = Groq(api_key=os.getenv("GROQ_API_KEY"))
ANAKIN_KEY = os.getenv("ANAKIN_API_KEY")

class Task(BaseModel):
    task_id: str
    abstract: str

@app.post("/run")
async def run(task: Task):
    async with httpx.AsyncClient() as client:
        r = await client.get(
            "https://api.anakin.io/v1/search",
            params={
                "q": f"{task.abstract[:100]} site:grants.gov OR site:nsf.gov OR site:nih.gov",
                "format": "markdown",
                "limit": 5
            },
            headers={"Authorization": f"Bearer {ANAKIN_KEY}"},
            timeout=30
        )
    raw = r.json().get("content", "No matching grants found")

    chat = llm.chat.completions.create(
        model="llama-3.1-8b-instant",
        messages=[
            {"role": "system", "content": """You are an expert grant discovery officer.
Extract top active grants from the search results. Return JSON:
{"grants": [{"title": "...", "agency": "...", "deadline": "...", "max_award": "...", "key_focus": "..."}]}"""},
            {"role": "user", "content": f"Project Abstract: {task.abstract}\n\nSearch results:\n{raw[:4000]}"}
        ],
        response_format={"type": "json_object"}
    )
    return {
        "task_id": task.task_id,
        "agent": "grant_scout",
        "result": chat.choices[0].message.content
    }

@app.get("/health")
def health(): return {"status": "ok", "agent": "grant_scout"}

```

**Compliance Analyst Agent (~80 lines):**

```python
# agents/compliance_analyst/main.py
from fastapi import FastAPI
from pydantic import BaseModel
import httpx, os
from groq import Groq

app = FastAPI()
llm = Groq(api_key=os.getenv("GROQ_API_KEY"))
ANAKIN_KEY = os.getenv("ANAKIN_API_KEY")

class Task(BaseModel):
    task_id: str
    abstract: str
    grant_target: str

@app.post("/run")
async def run(task: Task):
    async with httpx.AsyncClient() as client:
        r = await client.get(
            "https://api.anakin.io/v1/search",
            params={
                "q": f"{task.grant_target} eligibility requirements review criteria",
                "format": "markdown"
            },
            headers={"Authorization": f"Bearer {ANAKIN_KEY}"},
            timeout=30
        )
    raw = r.json().get("content", "")

    chat = llm.chat.completions.create(
        model="llama-3.1-8b-instant",
        messages=[
            {"role": "system", "content": """You are a grant compliance reviewer.
Analyze alignment between the project and grant requirements. Return JSON:
{"match_score_percentage": 85, "alignment_strengths": [...], "compliance_gaps": [...]}"""},
            {"role": "user", "content": f"Abstract: {task.abstract}\nGrant Details: {task.grant_target}\nGuidelines: {raw[:4000]}"}
        ],
        response_format={"type": "json_object"}
    )
    return {"task_id": task.task_id, "agent": "compliance_analyst", "result": chat.choices[0].message.content}

@app.get("/health")
def health(): return {"status": "ok", "agent": "compliance_analyst"}

```

**Synthesis Agent (Groq 70B for Strategic Writing):**

```python
# agents/synthesis/main.py
from fastapi import FastAPI
from pydantic import BaseModel
from groq import Groq
import os

app = FastAPI()
llm = Groq(api_key=os.getenv("GROQ_API_KEY"))

class SynthesisTask(BaseModel):
    task_id: str
    abstract: str
    grants: str
    compliance: str

@app.post("/run")
async def run(task: SynthesisTask):
    chat = llm.chat.completions.create(
        model="llama-3.3-70b-versatile",
        messages=[
            {"role": "system", "content": """You are a senior grant strategy director.
Synthesize the final funding brief and write an optimized proposal outline. Return JSON with:
{
  "best_recommendation": "Top grant option with rationale",
  "readiness_score": "e.g. 90%",
  "strategic_recommendations": ["3 actionable edits to win the grant"],
  "drafted_specific_aims": "Compelling 1-paragraph summary tailored to the funder rubric"
}"""},
            {"role": "user", "content": f"Project: {task.abstract}\nGrants Found: {task.grants}\nCompliance Analysis: {task.compliance}"}
        ],
        response_format={"type": "json_object"}
    )
    return {"task_id": task.task_id, "agent": "synthesis", "result": chat.choices[0].message.content}

@app.get("/health")
def health(): return {"status": "ok", "agent": "synthesis"}

```

### ⏱ Hour 8–12: Docker + Nasiko Registration

Containerize and deploy the agents to the Nasiko network, mirroring the reliable container setup from your original architecture.

### ⏱ Hour 12–17: DronaHQ Dashboard UI

Build an intuitive drag-and-drop UI layout in DronaHQ:

* **Input Panel:** Project Title & Abstract Textarea.
* **Status Panel:** Live tracking of *Grant Scout*, *Compliance Analyst*, and *Synthesis Agent*.
* **Report Panel:** Displays the best matched grant, funding limits, compliance percentage score, and the AI-generated strategy brief.

### ⏱ Hour 17–24: Test, Polish, & Demo Prep

* Test with real-world technical and research abstracts.
* Record a 90-second backup video showing the automated workflow from input to structured grant strategy brief.

---

## Why This Wins in the Innovation & Funding Domain

| Judging lens | GrantMatch AI advantage |
| --- | --- |
| **Real problem** | Securing funding is tedious, high-stakes, and universally painful for researchers and startups. |
| **AI novelty** | Multi-agent collaboration (Scout $\rightarrow$ Compliance $\rightarrow$ Synthesis) provides deeper oversight than a standard chatbot prompt.

 |
| **Live data** | Anakin.io pulls live open grants and active funding opportunities directly from agency websites.

 |
| **Speed** | Groq's high throughput produces the complete strategic breakdown in seconds.

 |

---