const http = require("http");

const API = "http://localhost:3000";

function post(path, body) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify(body);
    const url = new URL(path, API);
    const options = {
      hostname: url.hostname,
      port: url.port,
      path: url.pathname,
      method: "POST",
      headers: { "Content-Type": "application/json", "Content-Length": data.length },
    };
    const req = http.request(options, (res) => {
      let body = "";
      res.on("data", (chunk) => (body += chunk));
      res.on("end", () => resolve({ status: res.statusCode, data: JSON.parse(body) }));
    });
    req.on("error", reject);
    req.write(data);
    req.end();
  });
}

function get(path) {
  return new Promise((resolve, reject) => {
    http.get(`${API}${path}`, (res) => {
      let body = "";
      res.on("data", (chunk) => (body += chunk));
      res.on("end", () => resolve({ status: res.statusCode, data: JSON.parse(body) }));
    }).on("error", reject);
  });
}

function put(path, body) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify(body);
    const url = new URL(path, API);
    const options = {
      hostname: url.hostname,
      port: url.port,
      path: url.pathname,
      method: "PUT",
      headers: { "Content-Type": "application/json", "Content-Length": data.length },
    };
    const req = http.request(options, (res) => {
      let b = "";
      res.on("data", (chunk) => (b += chunk));
      res.on("end", () => resolve({ status: res.statusCode, data: JSON.parse(b) }));
    });
    req.on("error", reject);
    req.write(data);
    req.end();
  });
}

async function run() {
  console.log("=== PHASE 2 TEST: Rate Limiting & Queuing ===\n");

  // Step 1: Set strict rate limit (3 tokens, 1/sec refill) for testing
  console.log("1. Setting strict rate limit: 3 tokens, 1/sec refill...");
  const limitRes = await put("/api/limits/mock-analyzer", {
    maxTokens: 3,
    refillRate: 1,
    maxQueueSize: 5,
    maxWaitTime: 10000,
  });
  console.log("   Config:", JSON.stringify(limitRes.data.config));

  // Step 2: Flush cache to ensure fresh requests
  console.log("\n2. Flushing cache...");
  const flushRes = await fetch(`${API}/api/cache/flush`, { method: "DELETE" });
  console.log("   Done");

  // Step 3: Send 6 requests rapidly (3 should pass, next should queue)
  console.log("\n3. Sending 6 rapid requests to mock-analyzer...");
  const promises = [];
  for (let i = 0; i < 6; i++) {
    // Each request has a unique query so cache doesn't interfere
    promises.push(
      post("/api/process", { agent: "mock-analyzer", query: `Analyze text number ${i + 1}` })
        .then((res) => ({
          index: i + 1,
          status: res.status,
          cached: res.data.cached,
          queued: res.data.queued,
          latency: res.data.latency,
          error: res.data.error,
          queueWaitTime: res.data.queueWaitTime,
        }))
    );
  }

  const results = await Promise.all(promises);

  console.log("\n   Results:");
  results.forEach((r) => {
    const status = r.status === 429 ? "❌ REJECTED" : r.queued ? "⏳ QUEUED→OK" : "✅ ALLOWED";
    const extra = r.queueWaitTime ? ` (waited ${r.queueWaitTime}ms)` : "";
    console.log(`   Request ${r.index}: ${status} | ${r.latency || 0}ms${extra}`);
  });

  // Step 4: Check rate limiter stats
  console.log("\n4. Rate limiter stats:");
  const statsRes = await get("/api/limits/mock-analyzer");
  console.log("   ", JSON.stringify(statsRes.data.stats, null, 2));

  // Step 5: Check queue status
  console.log("\n5. Queue status:");
  const queueRes = await get("/api/queue/mock-analyzer");
  console.log("   ", JSON.stringify(queueRes.data, null, 2));

  // Step 6: Check cache stats
  console.log("\n6. Cache stats:");
  const cacheRes = await get("/api/cache/stats");
  console.log("   ", JSON.stringify(cacheRes.data, null, 2));

  console.log("\n=== PHASE 2 TEST COMPLETE ===");
}

run().catch(console.error);
