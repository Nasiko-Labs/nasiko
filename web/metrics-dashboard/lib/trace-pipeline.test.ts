import { describe, expect, it } from "vitest";
import {
  classifySpan,
  isInfrastructureSpan,
  spansToPipelineGraph,
} from "@/lib/trace-pipeline";

describe("trace-pipeline", () => {
  it("treats JSON-RPC handlers as infrastructure", () => {
    const span = {
      id: "1",
      name: "a2a.server.request_handlers.jsonrpc_handler.JSONRPCHandler.on_message_send",
    };
    expect(isInfrastructureSpan(span)).toBe(true);
    expect(classifySpan(span)).toBe("step");
  });

  it("classifies ChatCompletion as LLM", () => {
    expect(
      classifySpan({ id: "1", name: "ChatCompletion", span_kind: "LLM" }),
    ).toBe("llm");
  });

  it("builds user → OpenAI pipeline only", () => {
    const { nodes, edges } = spansToPipelineGraph(
      [
        {
          id: "root",
          span_id: "root",
          name: "a2a.server.request_handlers.jsonrpc_handler.JSONRPCHandler.on_message_send",
          status_code: "OK",
          latency_ms: 1900,
          children: [
            {
              id: "llm",
              span_id: "llm",
              name: "ChatCompletion",
              span_kind: "LLM",
              status_code: "OK",
              latency_ms: 1800,
              children: [
                {
                  id: "cb",
                  span_id: "cb",
                  name: "a2a.server.events.event_consumer.EventConsumer.agent_task_callback",
                  status_code: "OK",
                  latency_ms: 0,
                  children: [],
                },
              ],
            },
          ],
        },
      ],
      { userPreview: "Translate hello to French" },
    );

    expect(nodes).toHaveLength(2);
    expect(nodes[0].data.node_kind).toBe("user");
    expect(nodes[0].data.label).toContain("Translate");
    expect(nodes[1].data.node_kind).toBe("llm");
    expect(nodes[1].data.label).toBe("OpenAI");
    expect(nodes[1].data.subtitle).toBe("Chat completion");
    expect(edges).toHaveLength(1);
  });

  it("marks error tone on failed LLM", () => {
    const { nodes } = spansToPipelineGraph([
      {
        id: "llm",
        span_id: "llm",
        name: "ChatCompletion",
        span_kind: "LLM",
        status_code: "ERROR",
        children: [],
      },
    ]);
    expect(nodes.find((n) => n.data.node_kind === "llm")?.data.tone).toBe("error");
  });
});
