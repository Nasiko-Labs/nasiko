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

async function run() {
  console.log("=== PHASE 3 TEST: Agent Proxy & Discovery ===\n");

  // 1. List all agents
  console.log("1. Listing all agents...");
  const agents = await get("/api/agents");
  console.log(`   Total: ${agents.total} (${agents.mockAgents} mock, ${agents.realAgents} real)`);
  agents.agents.forEach((a) => {
    console.log(`   • ${a.name} [${a.type}] — ${a.description}`);
  });

  // 2. Health check mock agents
  console.log("\n2. Health checks...");
  for (const name of ["mock-translator", "mock-summarizer", "mock-analyzer"]) {
    const health = await get(`/api/agents/${name}/health`);
    console.log(`   ${name}: ${health.status} (${health.type})`);
  }

  // 3. Health check a real agent (may fail if not running)
  console.log("\n3. Real agent health check (translator)...");
  const realHealth = await get("/api/agents/translator/health");
  console.log(`   translator: ${realHealth.status} (${realHealth.type})`);
  if (realHealth.error) console.log(`   error: ${realHealth.error}`);

  console.log("\n=== PHASE 3 TEST COMPLETE ===");
}

run().catch(console.error);
