const http = require("http");
const API = "http://localhost:3000";

function get(path) {
  return new Promise((resolve, reject) => {
    http.get(`${API}${path}`, (res) => {
      let body = "";
      res.on("data", (chunk) => (body += chunk));
      res.on("end", () => resolve(JSON.parse(body)));
    }).on("error", reject);
  });
}

function post(path, body) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify(body);
    const url = new URL(path, API);
    const options = {
      hostname: url.hostname, port: url.port, path: url.pathname,
      method: "POST",
      headers: { "Content-Type": "application/json", "Content-Length": data.length },
    };
    const req = http.request(options, (res) => {
      let b = "";
      res.on("data", (chunk) => (b += chunk));
      res.on("end", () => resolve(JSON.parse(b)));
    });
    req.on("error", reject);
    req.write(data);
    req.end();
  });
}

function del(path) {
  return new Promise((resolve, reject) => {
    const url = new URL(path, API);
    const options = { hostname: url.hostname, port: url.port, path: url.pathname, method: "DELETE" };
    const req = http.request(options, (res) => {
      let b = "";
      res.on("data", (chunk) => (b += chunk));
      res.on("end", () => resolve(JSON.parse(b)));
    });
    req.on("error", reject);
    req.end();
  });
}

async function run() {
  console.log("=== PHASE 3+4 TEST: Full Flow with Stats ===\n");

  // Reset stats and flush cache
  await post("/api/stats/reset", {});
  await del("/api/cache/flush");

  // Send a few requests
  console.log("1. Sending 5 requests (mix of agents)...");
  await post("/api/process", { agent: "mock-translator", query: "Hello world" });
  await post("/api/process", { agent: "mock-analyzer", query: "Test sentiment" });
  await post("/api/process", { agent: "mock-summarizer", query: "Long text here for summarization" });
  // Repeat for cache hits
  await post("/api/process", { agent: "mock-translator", query: "Hello world" });
  await post("/api/process", { agent: "mock-analyzer", query: "Test sentiment" });
  console.log("   Done!\n");

  // Global stats
  console.log("2. Global stats:");
  const global = await get("/api/stats");
  console.log(`   Total requests: ${global.totalRequests}`);
  console.log(`   Cache hits: ${global.cacheHits}`);
  console.log(`   Cache hit rate: ${global.cacheHitRate}%`);
  console.log(`   Success rate: ${global.successRate}%`);

  // Per-agent stats
  console.log("\n3. Per-agent stats:");
  const agentStats = await get("/api/stats/agents");
  for (const [name, stats] of Object.entries(agentStats)) {
    console.log(`   ${name}: ${stats.totalRequests} requests, ${stats.cacheHitRate}% cache, avg ${stats.avgLatency}ms`);
  }

  // Recent feed
  console.log("\n4. Recent request feed:");
  const feed = await get("/api/stats/feed?count=5");
  feed.requests.forEach((r) => {
    const icon = r.cached ? "📦" : "🔄";
    console.log(`   ${icon} ${r.agent} | ${r.latency}ms | cached=${r.cached}`);
  });

  // Time series
  console.log("\n5. Time series data:");
  const ts = await get("/api/stats/history");
  console.log(`   ${ts.points} data points`);
  if (ts.series.length > 0) {
    const last = ts.series[ts.series.length - 1];
    console.log(`   Latest: ${last.requests} requests, ${last.cacheHits} cache hits, avg ${last.avgLatency}ms`);
  }

  console.log("\n=== PHASE 3+4 TEST COMPLETE ===");
}

run().catch(console.error);
