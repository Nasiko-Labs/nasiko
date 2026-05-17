import { describe, expect, it } from "vitest";
import { spansToFlowGraph } from "@/lib/trace-graph";

describe("spansToFlowGraph", () => {
  it("builds nodes and parent-child edges", () => {
    const { nodes, edges, truncated } = spansToFlowGraph([
      {
        id: "root",
        span_id: "root",
        name: "POST /",
        latency_ms: 100,
        status_code: "OK",
        children: [
          {
            id: "child",
            span_id: "child",
            name: "llm",
            latency_ms: 80,
            status_code: "OK",
            children: [],
          },
        ],
      },
    ]);

    expect(truncated).toBe(false);
    expect(nodes).toHaveLength(2);
    expect(edges).toHaveLength(1);
    expect(edges[0].source).toBe("root");
    expect(edges[0].target).toBe("child");
  });

  it("truncates very large trees", () => {
    const deep: Parameters<typeof spansToFlowGraph>[0] = [
      {
        id: "r",
        span_id: "r",
        name: "root",
        children: [],
      },
    ];
    let current = deep[0];
    for (let i = 0; i < 80; i++) {
      const child = {
        id: `c${i}`,
        span_id: `c${i}`,
        name: `span-${i}`,
        children: [] as never[],
      };
      current.children = [child];
      current = child;
    }

    const { truncated, nodes } = spansToFlowGraph(deep, 10);
    expect(truncated).toBe(true);
    expect(nodes.length).toBeLessThanOrEqual(10);
  });
});
