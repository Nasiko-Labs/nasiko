// Setup CLI — static content; scenarios capture each step tab.
export default {
  scenarios: {
    "step-cluster": async (page) => {
      await page.click('[data-step="cluster"]');
      await page.waitForSelector('[data-panel="cluster"].is-active');
      await page.waitForSelector('.step-tab.is-active[data-step="cluster"]');
      await page.waitForTimeout(450);
    },
    "step-agent": async (page) => {
      await page.click('[data-step="agent"]');
      await page.waitForSelector('[data-panel="agent"].is-active');
      await page.waitForTimeout(450);
    },
    "step-run": async (page) => {
      await page.click('[data-step="run"]');
      await page.waitForSelector('[data-panel="run"].is-active');
      await page.waitForTimeout(450);
    },
  },
};
