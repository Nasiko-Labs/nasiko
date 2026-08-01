export default {
  fetch: [
    ["POST /api/chat/sessions", { session_id: "s-preview-001", id: "s-preview-001" }],
    [{ method: "POST", path: /^\/api\/chat\/sessions\/.*\/messages$/ }, { ok: true }],
    [{ method: "GET", path: /^\/api\/chat\/sessions\/.*\/messages$/ }, {
      data: [
        { role: "user", content: "How do I configure container networking for multi-agent communication?", trace_id: null },
        { role: "assistant", content: "Here's how to configure container networking for multi-agent communication:\n\n**1. DNS-based discovery**\n\nEach agent container gets a DNS entry in the `nasiko-agents` network. Other agents can reach it via `<agent-id>.nasiko-agents.local`.\n\n**2. Network policy**\n\nBy default, agents can communicate freely within the same namespace. To restrict:\n\n- Use `network_policy: isolated` in the deploy spec\n- Whitelist specific agents with `allowed_peers`\n\n**3. Example configuration**\n\n```yaml\ndeploy:\n  network: nasiko-agents\n  network_policy: restricted\n  allowed_peers:\n    - coding-agent\n    - research-agent\n```\n\n**4. Verifying connectivity**\n\nYou can test connectivity between agents using:\n\n```bash\nnasiko exec <agent-id> -- curl http://other-agent.nasiko-agents.local:8080/health\n```\n\nSee the [networking docs](https://docs.nasiko.dev/networking) for more details.", trace_id: "abc123def456" },
        { role: "user", content: "Can I use mTLS between agents?", trace_id: null },
        // CLI-written sessions store replies as "agent" (not "assistant") —
        // the UI must render both as markdown agent replies.
        { role: "agent", content: "Yes, mTLS between agents is supported. Here's how:\n\n1. Enable the `mtls` feature on the namespace\n2. The platform auto-provisions certificates via the internal CA\n3. Agents receive certs as mounted secrets\n\nNo code changes needed in your agent -- the sidecar proxy handles TLS termination.\n\n`NASIKO_MTLS=enabled` in the agent's env vars activates it.", trace_id: "xyz789abc012" },
        { role: "user", content: "Compare the network policy modes in a table", trace_id: null },
        { role: "assistant", content: "Here's a comparison of the available network policy modes:\n\n| Mode | Default peers | Use case | Overhead |\n| --- | --- | --- | --- |\n| `open` | All agents in namespace | Development, trusted teams | None |\n| `restricted` | Only `allowed_peers` | Production multi-tenant | Low |\n| `isolated` | None | Sensitive workloads, compliance | Low |\n| `mtls` | `allowed_peers` + mutual TLS | Zero-trust environments | Medium |\n\nFor most production deployments, `restricted` is the sweet spot between security and operability.", trace_id: "tbl456trace789" },
      ],
    }],
    ["POST /api/orchestrator/a2a", (() => {
      const answer = "## Scaling agents\n\nUse the `scale` command with **replica count**:\n\n```bash\nnasiko scale my-agent --replicas 3\n```\n\n- Replicas share one service endpoint\n- Traffic is round-robin balanced";
      const evt = (obj) => `data: ${JSON.stringify({ result: obj })}\n\n`;
      const dataMsg = (data) => ({ status: { state: "TASK_STATE_WORKING", message: { parts: [{ data }] } } });
      return { __stream: [
        { text: evt({ statusUpdate: dataMsg({ type: "trace_meta", trace_id: "trace-preview-chat" }) }), delay: 600 },
        { text: evt({ statusUpdate: dataMsg({ type: "thinking", content: "Analyzing the request..." }) }), delay: 150 },
        { text: evt({ statusUpdate: dataMsg({ type: "tool_call", agent: "devops-agent", message: "How do I scale an agent deployment?", turn: 1 }) }), delay: 150 },
        { text: evt({ statusUpdate: dataMsg({ type: "sub_status", agent: "devops-agent", message: "Consulting deployment runbook..." }) }), delay: 100 },
        { text: evt({ statusUpdate: dataMsg({ type: "sub_status", agent: "devops-agent", message: "Checking KEDA autoscaler limits..." }) }), delay: 100 },
        { text: evt({ statusUpdate: dataMsg({ type: "tool_result", agent: "devops-agent", result: "Use `nasiko scale` with --replicas; traffic is balanced round-robin.", success: true, turn: 1 }) }), delay: 250 },
        // Working-status text chunks — cumulative sends, like python-SDK agents stream.
        { text: evt({ statusUpdate: { status: { state: "TASK_STATE_WORKING", message: { parts: [{ text: "## Scaling agents\n\nUse the `scale`" }] } } } }), delay: 150 },
        { text: evt({ statusUpdate: { status: { state: "TASK_STATE_WORKING", message: { parts: [{ text: "## Scaling agents\n\nUse the `scale` command with **replica count**:" }] } } } }), delay: 150 },
        { text: evt({ artifactUpdate: { artifact: { parts: [{ text: answer }] }, append: false } }), delay: 300 },
        { text: evt({ statusUpdate: { status: { state: "TASK_STATE_COMPLETED", message: { parts: [{ text: answer }] } } } }), delay: 100 },
      ] };
    })()],
  ],
  scenarios: {
    "with-messages": async (page) => {
      // Navigate with session_id to trigger message loading
      const url = page.url();
      const base = url.split('?')[0];
      await page.goto(`${base}?agent_id=a-001&agent_name=Coding+Agent&session_id=s-001`);
      await page.waitForSelector('.msg-row');
    },
    "hover-actions": async (page) => {
      // Hover an assistant reply to reveal the copy + trace actions toolbar
      const url = page.url();
      const base = url.split('?')[0];
      await page.goto(`${base}?agent_id=a-001&agent_name=Coding+Agent&session_id=s-001`);
      await page.waitForSelector('.msg-row.is-assistant');
      await page.hover('.msg-row.is-assistant:last-of-type .msg');
      await page.waitForTimeout(200);
    },
    "streamed-response": async (page) => {
      // Submit a message and let the mocked SSE stream render live
      await page.fill('#textarea', 'How do I scale an agent?');
      await page.click('#submitBtn');
      await page.waitForSelector('.stream-content.is-visible', { timeout: 8000 });
      await page.waitForTimeout(300);
    },
    // Just after submit: bouncing typing indicator, before any stream event.
    "typing-indicator": async (page) => {
      await page.fill('#textarea', 'How do I scale an agent?');
      await page.click('#submitBtn');
      await page.waitForSelector('.typing-indicator', { timeout: 5000 });
      await page.waitForTimeout(150);
    },
    // Mid-stream: tool-call step visible and running.
    "streaming-steps": async (page) => {
      await page.fill('#textarea', 'How do I scale an agent?');
      await page.click('#submitBtn');
      await page.waitForSelector('agent-steps .step', { timeout: 5000 });
      await page.waitForTimeout(250);
    },
    // Completed reply with the steps summary re-expanded.
    "steps-expanded": async (page) => {
      await page.fill('#textarea', 'How do I scale an agent?');
      await page.click('#submitBtn');
      await page.waitForSelector('.stream-content.is-visible', { timeout: 8000 });
      await page.click('agent-steps .steps-header');
      await page.waitForTimeout(200);
    },
  },
};
