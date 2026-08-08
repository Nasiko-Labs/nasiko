// Create-workflow fixtures — agents for the per-step picker + a canned
// /api/maf/generate plan (envelope {data,status_code,message}).
const plan = {
  name: "Social media content pipeline",
  description: "Draft three caption variations each weekday, review tone and brand compliance, then queue the approved posts for publishing.",
  output_generation: "Summarise which captions were approved and when they are scheduled.",
  steps: [
    { agent_id: "a-001", agent_name: "Content Writer Agent", task_description: "Generate three caption variations using the latest campaign brief and brand tone." },
    { agent_id: "a-004", agent_name: "Review Writer Agent", task_description: "Evaluate the generated captions for tone, grammar, and campaign alignment." },
    { agent_id: "a-005", agent_name: "Compliance Checker", task_description: "Check every claim against brand and legal guidelines, and flag anything unapproved." },
    { agent_id: "a-006", agent_name: "Publishing Agent", task_description: "Queue approved captions for publishing and record the schedule." },
  ],
};

export default {
  fetch: [
    [{ method: "GET", path: /^\/api\/agents/ }, {
      data: [
        { id: "a-001", name: "content-writer", display_name: "Content Writer Agent" },
        { id: "a-004", name: "review-writer", display_name: "Review Writer Agent" },
        { id: "a-005", name: "compliance-checker", display_name: "Compliance Checker" },
        { id: "a-006", name: "publishing-agent", display_name: "Publishing Agent" },
        { id: "a-002", name: "research-agent", display_name: "Research Agent" },
      ],
      total: 5,
    }],
    ["POST /api/maf/generate", { data: plan, status_code: 200, message: "ok" }],
  ],
  scenarios: {
    drafting: async (page) => {
      await page.evaluate(() => {
        window.generateWorkflow = () => new Promise(() => {}); // never settles
      });
      await page.fill("#wf-desc", "Draft three caption variations each weekday, review the tone, then queue the approved ones for publishing.");
      await page.click("#draft-btn");
      await page.waitForSelector(".drafting:not([hidden])");
      await page.waitForTimeout(300);
    },
    drafted: async (page) => {
      await page.fill("#wf-desc", "Draft three caption variations each weekday, review the tone, then queue the approved ones for publishing.");
      await page.click("#draft-btn");
      await page.waitForSelector("#drafted-note:not([hidden])");
      await page.waitForTimeout(300);
    },
    "no-agents": async (page) => {
      await page.evaluate(() => {
        window.generateWorkflow = async () => {
          const err = new Error("no agents registered — register at least one agent before generating a MAF");
          err.status = 400;
          throw err;
        };
      });
      await page.fill("#wf-desc", "Enrich incoming leads with company data.");
      await page.click("#draft-btn");
      await page.waitForSelector("#gen-notice:not([hidden])");
      await page.waitForTimeout(300);
    },
  },
};
