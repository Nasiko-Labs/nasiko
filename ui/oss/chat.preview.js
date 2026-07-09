export default {
  fetch: [
    ["POST /api/chat/sessions", { session_id: "s-preview-001", id: "s-preview-001" }],
    [{ method: "GET", path: /^\/api\/chat\/sessions\/.*\/messages$/ }, {
      data: [
        { role: "user", content: "How do I configure container networking for multi-agent communication?", trace_id: null },
        { role: "assistant", content: "Here's how to configure container networking for multi-agent communication:\n\n**1. DNS-based discovery**\n\nEach agent container gets a DNS entry in the `nasiko-agents` network. Other agents can reach it via `<agent-id>.nasiko-agents.local`.\n\n**2. Network policy**\n\nBy default, agents can communicate freely within the same namespace. To restrict:\n\n- Use `network_policy: isolated` in the deploy spec\n- Whitelist specific agents with `allowed_peers`\n\n**3. Example configuration**\n\n```yaml\ndeploy:\n  network: nasiko-agents\n  network_policy: restricted\n  allowed_peers:\n    - coding-agent\n    - research-agent\n```\n\n**4. Verifying connectivity**\n\nYou can test connectivity between agents using:\n\n```bash\nnasiko exec <agent-id> -- curl http://other-agent.nasiko-agents.local:8080/health\n```\n\nSee the [networking docs](https://docs.nasiko.dev/networking) for more details.", trace_id: "abc123def456" },
        { role: "user", content: "Can I use mTLS between agents?", trace_id: null },
        // CLI-written sessions store replies as "agent" (not "assistant") —
        // the UI must render both as markdown agent replies.
        { role: "agent", content: "Yes, mTLS between agents is supported. Here's how:\n\n1. Enable the `mtls` feature on the namespace\n2. The platform auto-provisions certificates via the internal CA\n3. Agents receive certs as mounted secrets\n\nNo code changes needed in your agent -- the sidecar proxy handles TLS termination.\n\n`NASIKO_MTLS=enabled` in the agent's env vars activates it.", trace_id: "xyz789abc012" },
      ],
    }],
    ["POST /api/orchestrator/a2a", (() => {
      const answer = "## Scaling agents\n\nUse the `scale` command with **replica count**:\n\n```bash\nnasiko scale my-agent --replicas 3\n```\n\n- Replicas share one service endpoint\n- Traffic is round-robin balanced";
      const evt = (obj) => `data: ${JSON.stringify({ result: obj })}\n\n`;
      return { __stream: [
        { text: evt({ statusUpdate: { status: { state: "TASK_STATE_WORKING", message: { parts: [{ data: { type: "thinking", content: "Analyzing the request..." } }] } } } }), delay: 200 },
        { text: evt({ artifactUpdate: { artifact: { parts: [{ text: answer }] }, append: false } }), delay: 500 },
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
    "streamed-response": async (page) => {
      // Submit a message and let the mocked SSE stream render live
      await page.fill('#textarea', 'How do I scale an agent?');
      await page.click('#submitBtn');
      await page.waitForSelector('.stream-content.is-visible', { timeout: 5000 });
      await page.waitForTimeout(300);
    },
  },
};
