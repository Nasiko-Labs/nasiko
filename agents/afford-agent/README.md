  # Afford Now: affordability-check A2A agent for Nasiko

  Answers "Can I afford this purchase right now?"

  ## How it works
  1. **Price**: a price the user states, else an Anakin web search of INR listings (labeled mock if Anakin fails).
  2. **Verdict**: a deterministic 90-day balance simulation with plan selection (`plan_selector.py`, `forecast_engine.py`), ported from an earlier tested project. The LLM never decides or calculates.
  3. **Explanation**: the LLM writes the plain-language answer from the tool outputs only.

  ## On Nasiko
  - A2A v1.0 agent deployed via the dashboard (Add Agent, Upload ZIP)
  - LLM calls go through Nasiko's LLM Router; the agent holds only a short-lived token
  - Anakin key stored as the encrypted per-agent secret `ANAKIN_API_KEY`
  - Every request is traced in the dashboard

  ## Limitations
  - Fixed demo spending profile (`src/demo_state.py`), not real account data
  - Picking the price among search results is LLM judgment; the verdict itself is deterministic
  - Output is guidance, not a guarantee