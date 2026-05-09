# Nasiko Request Layer

An **adaptive traffic-control layer** designed for resilient multi-agent orchestration. It sits directly in front of the agent fleet and provides intelligent caching, per-agent rate limiting, and request queuing, along with a live observability dashboard.

![Dashboard Preview](./dashboard/public/preview.png) *(Preview placeholder)*

## Features

- ⚡ **Intelligent Caching**: Redis-backed request caching with SHA-256 fingerprinting. Caches successful responses to bypass agent invocation for duplicate queries.
- 🚦 **Adaptive Rate Limiting**: Per-agent token bucket rate limiters to prevent any single agent from being overwhelmed.
- 🗄️ **Traffic Queuing**: When agents reach their rate limits, overflow traffic is safely queued (using BullMQ) and processed as tokens become available.
- 📊 **Live Observability**: Real-time Socket.io powered dashboard built with React and TailwindCSS. Watch throughput, queue depths, cache hit rates, and latency over time.
- 🛠️ **Test Panel**: Built-in test traffic generator to simulate load and watch the system adapt live.

## Quick Start (Local Development)

The project is split into two parts: the Express backend (`server`) and the React frontend (`dashboard`).

### 1. Start Redis
Make sure you have Redis running locally on port 6379.
```bash
docker run -p 6379:6379 -d redis
```

### 2. Start the Backend Server
```bash
cd server
npm install
npm run dev
```
The server will start on `http://localhost:3000` and connect to Redis.

### 3. Start the Dashboard
Open a new terminal:
```bash
cd dashboard
npm install
npm run dev
```
The dashboard will start on `http://localhost:5173`. Open this URL in your browser.

## Built-in Mock Agents
For testing without the full Nasiko Python backend, the server includes built-in mock agents:
- `mock-translator`: Simulates translation (800ms-2s latency)
- `mock-analyzer`: Simulates data analysis (500ms-1.5s latency)
- `mock-summarizer`: Simulates text summarization (1s-3s latency)

Use the Test Panel in the Dashboard to send traffic to these mock agents.

## Configuration
Edit `server/.env` to point to a different Redis instance or change the Kong Gateway URL when connecting to real agents.
