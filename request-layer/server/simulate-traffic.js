const http = require("http");

const API = "http://localhost:3000";

const agents = ["mock-translator", "mock-analyzer", "mock-summarizer"];
const queries = [
  "Hello world", "What is the weather?", "Analyze this document", 
  "Translate to French", "Summarize the report", "Extract key entities"
];

function post(agent, query) {
  const data = JSON.stringify({ agent, query });
  const url = new URL("/api/process", API);
  const options = {
    hostname: url.hostname, port: url.port, path: url.pathname,
    method: "POST",
    headers: { "Content-Type": "application/json", "Content-Length": data.length },
  };
  const req = http.request(options, (res) => {
    // just consume response
    res.on("data", () => {});
  });
  req.on("error", () => {});
  req.write(data);
  req.end();
}

console.log("🚦 Starting simulated traffic to dashboard...");

// Randomize traffic patterns
setInterval(() => {
  const numRequests = Math.floor(Math.random() * 5) + 1; // 1-5 requests per tick
  for(let i = 0; i < numRequests; i++) {
    const agent = agents[Math.floor(Math.random() * agents.length)];
    
    // High chance of repeated query to trigger cache hits (70%)
    const useRepeated = Math.random() > 0.3;
    const query = useRepeated 
      ? queries[Math.floor(Math.random() * 3)] // Only use first 3 for higher collision rate
      : queries[Math.floor(Math.random() * queries.length)] + " " + Date.now(); // Unique query
      
    post(agent, query);
  }
}, 500); // Pump traffic every 500ms

// Occasional traffic spikes to trigger rate limiting queues
setInterval(() => {
  console.log("🔥 Triggering traffic spike!");
  const agent = agents[Math.floor(Math.random() * agents.length)];
  for(let i = 0; i < 15; i++) {
    post(agent, `Spike query ${i}`);
  }
}, 10000); // Every 10 seconds

process.on('SIGINT', () => {
  console.log("Stopped traffic.");
  process.exit();
});
