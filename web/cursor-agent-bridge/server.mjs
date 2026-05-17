import cors from "cors";
import express from "express";
import { Agent } from "@cursor/sdk";

const app = express();
const port = Number(process.env.PORT || 8787);
const model = process.env.CURSOR_MODEL || "composer-2";
const cwd = process.env.CURSOR_AGENT_CWD || process.cwd();

app.use(cors());
app.use(express.json({ limit: "1mb" }));

function hasCursorKey() {
  return Boolean(process.env.CURSOR_API_KEY);
}

function summarizeContext(context = {}) {
  const agents = Array.isArray(context.agents) ? context.agents : [];
  const agentLines = agents
    .map(
      (agent) =>
        `- ${agent.name}: avg ${agent.avgResponseMs}ms, P95 ${agent.p95ResponseMs}ms, success ${agent.successCount}, errors ${agent.errorCount}, uptime ${agent.uptime}%, capacity ${agent.saturation}%, score ${agent.reliabilityScore}%`
    )
    .join("\n");

  return `
Telemetry mode: ${context.telemetryMode || "unknown"}
Telemetry note: ${context.telemetryReason || "not provided"}
Selected view: ${context.activeLabel || "All agents"}
Summary: avg ${context.summary?.avgResponseMs ?? "N/A"}ms, P95 ${context.summary?.p95ResponseMs ?? "N/A"}ms, success ${context.summary?.successCount ?? "N/A"}, errors ${context.summary?.errorCount ?? "N/A"}, uptime ${context.summary?.uptime ?? "N/A"}%, error rate ${context.summary?.errorRate ?? "N/A"}%, capacity ${context.summary?.saturation ?? "N/A"}%, reliability ${context.summary?.reliabilityScore ?? "N/A"}%.
Agents:
${agentLines || "- No agents available"}
`.trim();
}

app.get("/health", (_req, res) => {
  if (!hasCursorKey()) {
    res.status(503).json({
      status: "missing_key",
      message: "Set CURSOR_API_KEY and restart the bridge.",
    });
    return;
  }

  res.json({
    status: "ready",
    model,
  });
});

app.post("/metrics-agent", async (req, res) => {
  if (!hasCursorKey()) {
    res.status(503).json({
      error: "missing_cursor_api_key",
      message: "Set CURSOR_API_KEY and restart the bridge.",
    });
    return;
  }

  const question = String(req.body?.question || "").trim();
  const context = req.body?.context || {};

  if (!question) {
    res.status(400).json({ error: "missing_question" });
    return;
  }

  try {
    const agent = await Agent.create({
      apiKey: process.env.CURSOR_API_KEY,
      model: { id: model },
      local: { cwd },
    });

    const run = await agent.send(`
You are the Nasiko Assistant inside the Challenge 2 metrics dashboard.
Act as a friendly representative of Nasiko.
You can answer normal conversational questions, but your strongest expertise is Nasiko, the current dashboard, Challenge 2, agents, observability, and metrics.
Use the dashboard context for any metrics answer.
Do not invent private company details, secrets, or backend data that is not in the context.
Do not narrate your analysis. Do not say "the user wants" or "the context does not define" unless the user explicitly asks about missing context.
Answer directly in first person as the Nasiko Assistant.
If asked what Nasiko is, say that Nasiko is an AI agent platform for building, deploying, routing, and monitoring AI agents, and this dashboard shows the performance metrics for those agents.
If a question is general, answer naturally and briefly, then offer to connect it back to Nasiko or the current metrics.
If the question asks what to show judges, mention per-agent filtering, average response time, success count, error count, uptime percentage, telemetry mode, responsiveness, and the 24-hour charts.

Question:
${question}

Dashboard context:
${summarizeContext(context)}
`);

    const result = await run.wait();
    const answer = result.result || run.result || "";

    res.json({
      answer: answer.trim() || "I reviewed the metrics, but Cursor returned an empty response.",
      model,
    });
  } catch (error) {
    res.status(502).json({
      error: "cursor_agent_failed",
      message: error instanceof Error ? error.message : "Cursor agent request failed.",
    });
  }
});

app.listen(port, () => {
  console.log(`Nasiko Metrics Cursor bridge listening on http://127.0.0.1:${port}`);
  console.log(hasCursorKey() ? "Cursor bridge ready." : "Waiting for CURSOR_API_KEY.");
});
