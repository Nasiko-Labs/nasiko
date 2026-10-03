# Build the request classifier for the P2 hackathon track

Create an optional classifier that reads a request and its context, then returns the task type, difficulty, and confidence. Use Jev 1.13 through OpenRouter as the first model choice. Keep the existing keyword classifier as the default and backup. Add an optional rule that checks whether a model change is worth losing cached-token savings.

This is an implementation plan, not a report of completed work. No improvement, cost saving, or qualifying result has been measured yet. The planning work does not make paid model calls.

For a diagram and a short organiser demo script, read the [proposed solution flow](/home/chaitanya/projects/nasiko/nasiko_hackthon/llm-router/docs/classifier-demo-flow.md).

## How to read this

Follow the steps in order. One box is one unit of work. Each step names the evidence needed to finish it. Check a box only when its evidence exists.

The boxes start unchecked because the classifier has not been built. A passing code test is not enough. Also run the real model, run the real router, and measure speed and cost.

Submit one contribution for the classifier track. Finish it ready for review. Publishing or merging the contribution is a separate action.

## Check the rules before starting

- [ ] Read the common rules, P2 requirements, and demo checklist in the [hackathon brief](/home/chaitanya/projects/nasiko/hackathon-problems.md).
- [ ] Record the starting code version. This plan was grounded in commit `796211c2`. Check for newer changes before implementation.
- [ ] Keep implementation changes inside `llm-router/`, including its tests, example, data, and documentation.
- [ ] Use `[classifier]` at the start of the submitted change's title. Use `classifier`, rather than P2, in new code names.
- [ ] Keep the keyword classifier as the default. Enable Jev only when the user chooses it.
- [ ] Keep keys, private instructions, and sensitive user data out of submitted files.
- [ ] Submit a pull request from your own copy of the repository to `Nasiko-Labs/nasiko`. A pull request is the proposed code change the organisers review.
- [ ] Do not create a `submissions/` folder. The pull request itself is the submission.

Evidence comes from the [common rules](/home/chaitanya/projects/nasiko/hackathon-problems.md:3).

## Step 1. Record what already works

- [ ] Run the existing classifier example on the public sample and keep its output as the starting comparison.
- [ ] Run the existing routing tests before making changes.
- [ ] Record the existing category results, elapsed time, and test results.
- [ ] Confirm where the router reads the latest user request and chooses a model.

The existing keyword classifier chooses a category from the request text. It does not read context or predict difficulty and confidence.

The latest example reads the test file and writes one result per request. Its difficulty and confidence are currently empty. Extend this example rather than build a second evaluation entry point.

The router already chooses models at allowed points, keeps earlier choices for continuing work, and looks up models separately for each provider.

Finish when the original example runs and its results are saved. Treat an existing failure as a problem to investigate, not as a result of our changes.

## Step 2. Check Jev before depending on it

- [ ] Replace the key shared in chat with a new key before live testing. Keep it only in the local `.env` file.
- [ ] Load the local settings into the process when running examples. Saving a `.env` file alone does not make the current code read it.
- [ ] Call `typesafe/jev-1.13` through `https://openrouter.ai/api/alpha/decisions`.
- [ ] Send one request and its context. Ask two questions in the same call, one for task type and one for difficulty.
- [ ] Inspect the returned choices, probability values, model version, elapsed time, and reported cost.
- [ ] Repeat the same request in separate fresh runs. Include requests that are clear and requests that are ambiguous.
- [ ] Check that the organisers can allow this exact address through their permitted network connection.

Jev uses the Decisions API, which is the address for structured choices. It does not use the normal chat address. Our code must use the documented request and response fields.

OpenRouter's Jev tutorial says probabilities can differ between runs. Pinning the model version alone does not prove repeatability. Check whether this service offers an effective repeatability setting. Do not assume a chat model's randomness settings work with Jev.

Finish when real calls prove that the required outputs are available and repeated classification results are suitable for the hackathon's repeatability checks.

If they are not repeatable, record the differences and stop treating Jev as a confirmed choice. Investigate a supported control or a different permitted model. Do not hide differences with saved answers tied to test cases. A keyword-only run does not replace the required model comparison.

This is the first major decision point in the plan.

## Step 3. Write the task and difficulty rules

- [ ] Define the seven allowed task types using the table below.
- [ ] Rate the work the user actually requests. Ignore misleading keywords in quotations and examples.
- [ ] For a request with several tasks, choose the main requested outcome. Write down how ties are resolved.
- [ ] Use context to understand the request, not just the number of words.
- [ ] Keep these rules in submitted documentation and in the questions sent to Jev.

| Required name | Meaning in plain English |
| --- | --- |
| `code_generation` | Create or change code, including a small edit to a code comment. |
| `code_understanding` | Explain or interpret the supplied code. |
| `technical_design` | Plan how software parts should work together. |
| `analytical_reasoning` | Work through evidence, calculations, causes, or competing explanations. |
| `writing` | Write, rewrite, or summarise text for an audience. |
| `factual_lookup` | Give a known fact, including a short question about a programming feature without supplied code. |
| `general` | Handle a request that does not fit the other categories. |

Use the public sample's difficulty rules as the starting point.

| Difficulty | Meaning in plain English |
| --- | --- |
| 1 | One tiny operation, such as changing a word. |
| 2 | Straightforward work with few conditions. |
| 3 | Several steps with a limited set of conditions. |
| 4 | Substantial reasoning or design. |
| 5 | Reasoning across several connected parts, with careful checks. |

Ask Jev to choose one of five named difficulty options and convert that option to the corresponding whole number. This avoids guessing how to round a fractional score.

Finish when every example can be labelled using written rules. Keep examples whose labels are disputed and record the decision.

## Step 4. Create our own labelled examples

- [ ] Create a starting target of 70 learning examples and 35 separate checking examples. These counts are our plan, not organiser requirements.
- [ ] Include all seven categories and all five difficulty levels across the collection.
- [ ] Include ambiguous requests, several tasks in one request, reworded requests, unfamiliar topics, spelling mistakes, and irrelevant extra text.
- [ ] Include easy requests with difficult-sounding keywords and difficult requests with short descriptions.
- [ ] Include pairs where the same request means different work because the context changes.
- [ ] Label the examples using Step 3. Review generated labels before accepting them.
- [ ] Keep reworded copies and examples built from the same underlying situation in one group.
- [ ] Put each group entirely in the learning set or entirely in the checking set.
- [ ] Save the split before adjusting the classifier. Do not use checking labels to improve its questions or confidence rule.

Use the learning set to refine the questions and confidence rules. We do not need to train a language model from scratch. Document that the model was already trained and that our learning data guides our task rules and settings.

The checking set measures performance on examples not used for those adjustments. If results cause us to change the rules, use a new untouched checking set before claiming a final result.

Keep the organiser's 10 public examples separate. They check that the submission runs. They are not proof of improvement.

Finish when both sets exist, their labels have been reviewed, and similar copies do not cross between sets.

The [brief requires labelled training and validation data](/home/chaitanya/projects/nasiko/hackathon-problems.md:218), regardless of whether the model is local or hosted.

## Step 5. Give both classifiers the same input and output

- [ ] Add one shared way to ask for classification in `llm-router/src/routing/classifier.rs`.
- [ ] Accept the request and optional context.
- [ ] Return a valid task type, difficulty from 1 to 5, and confidence from 0 to 1.
- [ ] Wrap the existing keyword function without changing its category choices.
- [ ] Choose and document fixed difficulty and confidence values for the keyword version. State that these values are constants, not model predictions.
- [ ] Have the router hold one chosen classifier that can be replaced through settings.
- [ ] Pass configuration into classification code. Do not let model code read environment settings itself.

Finish when the keyword version matches the saved original category results, including the default router behaviour.

The required code name is `RequestClassifier`. The name identifies the shared classifier interface in the brief. It must support calls that wait for a remote service.

## Step 6. Add the Jev connection and check every response

- [ ] Use one shared HTTP client, which is the component that sends requests to OpenRouter.
- [ ] Send only the request and the relevant available context. Do not send answer labels, sample IDs, or suggested model tiers.
- [ ] Include the seven category definitions and five difficulty definitions.
- [ ] Check that both returned choices are from our allowed lists.
- [ ] Reject missing choices, unknown categories, invalid numbers, and incomplete responses.
- [ ] Record model version and returned usage without printing keys or sensitive request content.
- [ ] Let failures reach the shared backup handling in Step 7.

Use the returned probability for the chosen task category as the starting confidence measure. Check that it exists and lies between 0 and 1. If Jev does not supply usable probability values, use the backup rather than invent a confidence score.

Do not assume Jev's separate confidence field means the probability of a correct task label. Its tutorial describes that field as how concentrated the alternatives are.

Use the learning examples to check whether higher reported probabilities actually correspond to more correct answers. Freeze any adjustment before running the separate checking examples. Report the exact confidence meaning.

Finish when real and deliberately broken responses produce the expected valid result or backup action.

## Step 7. Keep failures safe and visible

- [ ] Use the keyword classifier if Jev cannot start, the key is missing, the connection fails, the service returns an error, or the response is invalid.
- [ ] Set a proposed three-second total time limit for each Jev classification. Make it configurable and check it with real timing.
- [ ] Count every backup use and record its reason.
- [ ] Choose the low-confidence cutoff from learning results. Document the chosen value and how it was selected.
- [ ] Below that cutoff, use the keyword result and retain the current conversation model. If no current model is known, use the configured answer model.
- [ ] Apply the same rule to missing confidence. Distinguish this case from a connection error.
- [ ] Do not automatically retry failures during routing unless retries fit inside the same total time limit.

A failed classification must not fail the user's entire request. A low-confidence decision must not silently send difficult work to a cheaper model.

Finish when forced errors, invalid replies, and time limits all use the backup. Low-confidence cases must keep the current model, or the configured model when no current model is known, and be counted separately.

## Step 8. Connect the classifier to the real router

- [ ] Call the classifier only when a new routing decision is allowed.
- [ ] Use the existing `cold_start` and `switch` rules. These mean starting routing or reaching an allowed switching point.
- [ ] On `continue`, keep the selected model unchanged and make no new Jev call.
- [ ] Respect models that users have fixed explicitly.
- [ ] Keep existing provider-specific tier lookups and user overrides.
- [ ] Pass available text context from supported incoming request formats through to the classifier. The current request-only path is not sufficient.
- [ ] Define which earlier messages, task instructions, and supplied text form the context. Use a fixed order and a documented size limit.
- [ ] Keep the answering model's full required prompt, tools, and conversation intact. Limiting the text sent to Jev must not remove information from the answering model.
- [ ] Test both the chat request path and the Responses request path if both are changed. Document other unsupported context forms.
- [ ] Preserve the existing default route when Jev is disabled.

For the first implementation, use the predicted task type in the existing tier-choice calculation. Return and evaluate difficulty, but do not assume it directly proves which model tier is safe.

Keep the existing learned quality records keyed by provider, task type, and tier. Do not add difficulty to that key in this plan. If that choice changes, add the mapping rules and a test of how feedback changes future choices, as required by the brief.

The current router makes random tier choices. Keep that behaviour when the new feature is off. For the enabled model path, use an exposed fixed seed and stable ordering of the stored quality values. Check that identical inputs and stored state produce the same tier.

Finish when a request travels through the real router, invokes the selected classifier at the allowed point, and leaves the selected model unchanged during tool steps.

Do not move classification into the component that coordinates agents.

## Step 8a. Decide whether switching is worth losing cached-token savings

An allowed switching point means we may reconsider the model. It does not mean we must change models. Jev describes the task. Nasiko chooses a candidate and then decides whether to keep the current model or switch.

Keep this implementation basic for the hackathon. Reuse existing token records and prices. Estimate the next request only. Do not build a dashboard, a new pricing service, or predictions about an entire future conversation.

### Remember the current model across user turns

- [ ] Add an optional setting for this switching rule. Leave existing behaviour unchanged when it is disabled.
- [ ] Keep a separate record for each user, agent, and conversation. Record the current provider and model, tier, last completed request, and last usage update time.
- [ ] Use an existing trusted conversation identity where available. Do not use the agent identity alone because one agent can serve several conversations.
- [ ] For coding agents, check whether a stable conversation identity reaches the router. Their current routing identity changes with each new user turn.
- [ ] If a trusted conversation identity is unavailable, use the configured model and record that switching comparison is unavailable. Do not guess a shared identity from similar prompt text.
- [ ] Preserve the existing record that holds a model choice during one tool loop. The longer conversation record supports comparison across new user turns.
- [ ] Update usage observations after a response completes, including streaming responses. Do not let an older response overwrite a newer model choice.
- [ ] Bound stored records by size and expiry. For a single local hackathon process, an in-memory store is acceptable. Document that a restart loses observations and that separate processes do not share them.
- [ ] On restart or missing records, use the configured model as the known starting choice. Do not claim continuity that was not recorded.

Finish when two separate conversations stay separate, while successive user turns in one conversation can find the current model.

### Reuse the recorded token and price information

- [ ] Read fresh input, output, cache reads, and cache creation from the provider's response before the existing background database write.
- [ ] Keep the existing token-usage logging. Use database history only when it can be linked reliably to the same conversation and model.
- [ ] Preserve whether counts were actually supplied. Existing database zero values alone cannot prove that usage was known.
- [ ] Reuse Nasiko's existing pricing engine for both models, including cache prices. Do not change the pricing package outside the allowed submission scope.
- [ ] Record whether a price is a current lookup or an estimate. Missing or unsuitable prices block a price-driven switch.
- [ ] Estimate input tokens from the actual outgoing prompt, including stable instructions, history, and tool definitions. Use a supported token counter where available. Clearly label approximations.
- [ ] Use recent completed output length as a basic estimate of the next answer length. Compare both models using the same expected answer length and record the assumption.
- [ ] Check that previously reused prompt text still appears unchanged at the beginning of the outgoing prompt. Keep provider and serving-route differences in the observation.
- [ ] Treat old usage, changed prompt structure, changed serving route, unknown cache lifetime, and unavailable cache data as uncertain reuse.

Reported cache reads describe the previous request. They do not reveal every token still available in the provider's cache or guarantee the next cache hit. Keeping the same model improves the opportunity for reuse, but does not control the provider's cache.

For uncertain reuse, calculate a range of staying costs, from plausible cached reuse to no reuse. If a candidate is not cheaper across that range, keep the current model. Do not turn unavailable cache evidence into a claim that staying has no discount.

### Compare the next request's estimated cost

Use the following terms in the comparison. All numbers are estimates until the response returns.

| Option | Include in the estimate |
| --- | --- |
| Stay | Fresh input at the current model's normal price, reusable input at its cached-read price, any cache creation, and expected output. |
| Switch | The full prompt at the candidate's normal input price, expected output, and cache creation when required by that provider. Assume no reuse on the candidate for this basic version. |

Do not charge the same input token as both fresh input and cache creation. Follow the existing pricing engine's counting rules.

- [ ] Calculate both options through the same pricing engine.
- [ ] Calculate savings as staying cost minus switching cost.
- [ ] Start with a configurable proposed 20 percent savings margin for price-driven switches. This is our rule, not a hackathon requirement.
- [ ] Switch for price only when savings exceed that margin and a small configurable absolute amount. Choose and record the absolute amount from the prices and examples used in the demo.
- [ ] Under uncertain cache reuse, require the margin against the cheapest plausible staying estimate.
- [ ] Keep the current model when the estimates are tied, savings are small, confidence is low, prices are unknown, or the current and candidate models are the same.
- [ ] Check that the candidate meets the task's documented quality requirement before considering a cheaper switch.
- [ ] Permit a stronger model for a quality reason at an allowed boundary. Record it as a quality-driven switch, not a cost saving.
- [ ] Do not treat difficulty alone as proof of model capability. Use the existing tier rules and documented checks to justify a stronger candidate.
- [ ] On `continue`, keep the current model and make no new classifier or cost-comparison call.
- [ ] Never claim to preserve tool continuation when its model identity cannot be recovered. Record that limitation in tests and the demo.

Jev's call cost belongs to both options once the call has happened. It therefore cancels out of the stay-versus-switch comparison. Include it in the total cost report and when comparing this router with a router that makes no Jev call.

Use this illustrative example to test the decision. These are invented prices for a test, not measured provider results.

| Case | Estimated stay cost | Estimated switch cost | Expected decision |
| --- | --- | --- | --- |
| Cache discount makes staying cheaper | $0.012 | $0.015 | Keep the current model. |
| Candidate is meaningfully cheaper and suitable | $0.012 | $0.006 | Switch if both savings margins pass. |
| Candidate is only slightly cheaper | $0.012 | $0.0115 | Keep the current model. |
| Prices or conversation history are unknown | Unknown | Unknown | Keep the current model, or use the configured model if none is known. |
| Stronger model is needed for acceptable quality | Lower | Higher | Switch only with a recorded quality reason. |

### Test the rule and show what it proves

- [ ] Test all five examples above using fixed prices and token counts.
- [ ] Test unchanged input with useful cache reads, changed input, expired observations, and no provider cache information.
- [ ] Test separate conversations, overlapping requests, and an older response completing after a newer one.
- [ ] Test a real new user turn that keeps the current model despite a cheaper-looking candidate.
- [ ] Test a case that switches when the complete estimated cost is meaningfully lower.
- [ ] Test that `continue` makes no model change and no Jev call.
- [ ] Record current model, candidate, both estimates, confidence, reuse assumptions, decision, and reason without saving sensitive prompt text.
- [ ] After each real response, record the actual token split and calculated cost. Compare the chosen option's estimate with its observed cost.
- [ ] Do not call the unchosen option's estimated cost an actual saving. It was not run.
- [ ] Run separate matched conversations for the original router, classifier-only routing, and classifier routing with this switching rule when claiming real benefit.
- [ ] Compare actual model changes, cached-token share, full token cost, Jev cost, decision time, and answer quality.
- [ ] Describe fixed-input tests as proof of the decision rule. Describe real-provider tests as evidence of actual cache and cost behaviour.

Finish when the real router uses the comparison at an allowed switching point and the tests prove that cheap model prices alone do not trigger a change.

## Step 9. Extend the organisers' evaluation command

- [ ] Extend `llm-router/examples/classifier_eval.rs`. Do not replace the required command.
- [ ] Read examples from the file named by `EVAL_SET`.
- [ ] Write one result line per input case to the file named by `OUT`, in input order.
- [ ] Write task type, whole-number difficulty, confidence, and elapsed microseconds.
- [ ] Use the same chosen classifier and backup handling as the router.
- [ ] Prepare the client or local model once before the loop.
- [ ] Include network time and failed-call time before backup in per-case timing.
- [ ] Return exit code zero when the evaluation completed. Do not treat zero as proof that results passed scoring.
- [ ] Keep aggregate scores and fallback details separate from the required prediction file.
- [ ] Run without required command-line arguments.
- [ ] Check the complete run stays below 15 minutes on an ordinary CPU machine.

A proposed three-second limit for roughly 200 sequential classifications uses at most roughly ten minutes waiting for classification calls. This leaves time for processing, but does not prove the complete command meets the limit. Measure the whole run, including startup.

The organisers compare results from two runs. Compare task type, difficulty, confidence, and selected tier across fresh runs. Report timing differences separately because real elapsed times naturally vary. Ask the organisers how their file comparison handles timing if their scoring instructions do not clarify it.

Do not claim repeatability from a saved response or from repeated calls inside one process alone.

Finish when both keyword and Jev modes complete the same command with the required output shape.

## Step 10. Measure results and report failures

- [ ] Run the keyword version and Jev version on exactly the same untouched checking set.
- [ ] Count correct task labels overall and within each category.
- [ ] Report how often the difficulty number is correct and how far wrong the other predictions are.
- [ ] Compare reported confidence against actual correctness. Include confident wrong answers.
- [ ] Report all backup uses, with low confidence, connection failures, and time limits separated.
- [ ] Report the middle decision time and the time within which 95 percent of decisions finish. These are the required p50 and p95 figures.
- [ ] Record the machine, setup time, number of real hosted calls, and full evaluation time.
- [ ] Sum returned call costs. Include paid calls that later use the backup.
- [ ] If cost information is missing, report it as unknown or clearly label a price-based estimate. Never treat a missing cost as zero.
- [ ] Show repeated-run differences and report variation if any choices use randomness.
- [ ] Report the public sample separately from our own checking set.
- [ ] Report the switching-rule comparisons from Step 8a separately from classification accuracy. Include how often models changed and how much input the provider reported as cached.

Do not claim that the new classifier saves money or improves answer quality from category accuracy alone.

To make either claim, run the original and changed routers on the same workloads. Compare final answer quality, chosen models, input and output tokens, and total cost including classification. Otherwise limit the claim to measured classification results.

Finish when every reported number has saved run evidence. Negative results are valid results and must remain visible.

## Step 11. Prepare the submitted change and demo

- [ ] Run the focused tests, formatting checks, and build checks.
- [ ] Confirm that only allowed files appear in the submitted change.
- [ ] Confirm that `.env` and the key are excluded from Git and from documentation.
- [ ] Document every required setting and the exact OpenRouter address.
- [ ] State that the organisers supply their own key and private test set.
- [ ] Include the model ID, actual returned model version, measured results, and unsupported cases.
- [ ] Start the title with `[classifier]`.
- [ ] Demonstrate the keyword version and the new version.
- [ ] Show one successful classification and one real failure or deliberately forced backup.
- [ ] Show decision times and cost per decision.
- [ ] Show a cache-aware keep decision and a beneficial estimated switch. Label controlled examples separately from actual provider measurements.
- [ ] Explain what is complete and what remains before production use.

The organisers use their own scorer and about 200 private requests. Their numbers decide ranking. The brief gives no mandatory accuracy percentage and no mandatory token-saving target.

Judging gives 40 points for readiness to merge, 25 for usefulness, 20 for code, tests, and docs, and 15 for the demo. First-time contributors receive a bonus. Completing this checklist does not guarantee acceptance or a winning score.

## Use the two-hour budget as a checkpoint

These are planning estimates, not measured completion times. Begin after replacing the exposed key, getting the local repository running, and preparing the Rust build tools.

The added switching rule makes two hours a target for a basic working demonstration, not a promise of a fully verified submission. Build the required classifier and evaluation first. Add the next-request cost comparison with fixed-input tests. Complete live conversation comparisons before claiming actual savings. Do not drop required labelled data, fallback tests, or evaluation work to fit the clock.

| Time | Work | Evidence before proceeding |
| --- | --- | --- |
| 0 to 15 minutes | Save the keyword comparison and test Jev availability and repeatability. | Original results and repeated live-call records. |
| 15 to 35 minutes | Write label rules, prepare labelled groups, and freeze the learning and checking split. | Reviewed data files and split record. |
| 35 to 60 minutes | Build the shared classifier, Jev connection, settings, and backup rules. | Passing focused tests and a valid real response. |
| 60 to 90 minutes | Connect prompt and context, conversation tracking, basic stay-or-switch comparison, and the evaluation command. | Router check, fixed-cost decision tests, and continuation checks. |
| 90 to 110 minutes | Run classifier comparisons, repeatability checks, and a cache-aware routing demonstration. | Saved predictions, decision records, and measured costs where available. |
| 110 to 120 minutes | Check scope, prepare instructions, and rehearse the demo. | Review-ready files and demo evidence. |

At minute 15, reconsider Jev if its outputs cannot meet the required repeatability checks.

At minute 60, check that both classifier choices and backup behaviour actually work. At minute 90, check the real router and evaluation command. If stable conversation tracking or prices are unavailable, record the limitation and keep the configured model. Do not present a fixed-input cost example as completed live routing.

If a required check remains unfinished at minute 120, report the exact unfinished work and continue if time permits. Do not replace missing evidence with a claim of completion. Preparing and reviewing enough labelled data may take longer than this budget.

Do not spend this window on optional tool selection, stop decisions, reasoning budgets, agent redesign, or dashboards.

## Appendix A. Identify the files to change

The following names are file addresses and required code names, rather than additional concepts to learn.

| File | Planned work |
| --- | --- |
| `llm-router/src/routing/classifier.rs` | Add the shared classifier contract, keyword wrapper, result rules, and backup handling. |
| `llm-router/src/routing/jev.rs` | Add the OpenRouter Decisions connection. This is a proposed new file. |
| `llm-router/src/routing/mod.rs` | Use the chosen classifier and stay-or-switch rule at existing allowed routing points. |
| `llm-router/src/routing/switch_policy.rs` | Compare estimated staying and switching costs. Proposed new file. |
| `llm-router/src/routing/cache.rs` | Add bounded conversation observations separately from existing tool-loop choices. |
| `llm-router/src/routing/boundary.rs` | Check available conversation identity without changing default behaviour or inventing a new gateway protocol. |
| `llm-router/src/usage.rs` | Feed reported usage to conversation observations while preserving existing logging. |
| `llm-router/src/config.rs` | Read classifier choice, model ID, address, key, time limit, confidence cutoff, random seed, and optional switching-rule settings. |
| `llm-router/src/lib.rs` | Hold the selected classifier and shared resources in the router's existing state. |
| `llm-router/src/bin/llm-router.rs` | Check startup settings for the standalone router. |
| `llm-router/src/handlers/chat.rs` | Pass relevant request context to routing. |
| `llm-router/src/handlers/responses.rs` | Pass relevant text context for Responses requests. |
| `llm-router/examples/classifier_eval.rs` | Keep the organisers' command and call the shared classifier. |
| `llm-router/tests/classifier_backend.rs` | Test valid decisions, broken replies, errors, time limits, and missing confidence. Proposed new file. |
| `llm-router/tests/router_e2e.rs` | Test actual classifier invocation, conversation separation, keep-or-switch outcomes, and no calls during continuation. |
| `llm-router/tests/switch_policy.rs` | Test price comparisons, missing information, cache uncertainty, and savings margins. Proposed new file. |
| `llm-router/tests/data/classifier/` | Store reviewed learning and checking examples and their group assignments. Proposed folder. |
| `llm-router/docs/` | Store labelling rules, run instructions, and the measured result report. |

Use existing dependencies where they already provide the needed functions. Change `llm-router/Cargo.toml` only if a required dependency is missing. Recheck scope before changing any file outside this table.

## Appendix B. Keep settings and commands reproducible

These are proposed setting names, not settings already implemented.

| Setting | Purpose |
| --- | --- |
| `CLASSIFIER_BACKEND` | Select `regex` for keywords or `jev` for Jev. Default to keywords. |
| `CLASSIFIER_MODEL` | Select `typesafe/jev-1.13`. |
| `CLASSIFIER_ENDPOINT` | Select the documented Decisions address or the organisers' compatible forwarding address. |
| `OPENROUTER_API_KEY` | Supply the local key. Organisers supply their own equivalent setting. |
| `CLASSIFIER_TIMEOUT_MS` | Set the proposed 3000 millisecond total time limit. |
| `CLASSIFIER_MIN_CONFIDENCE` | Set the cutoff selected from learning data. |
| `CLASSIFIER_SEED` | Fix the enabled router's random tier choices. |
| `CLASSIFIER_CACHE_SWITCH_ENABLED` | Enable the optional stay-or-switch comparison. Off by default. |
| `CLASSIFIER_SWITCH_MIN_SAVINGS_PCT` | Set the proposed 20 percent minimum savings for a price-driven switch. |
| `CLASSIFIER_SWITCH_MIN_SAVINGS_USD` | Set the minimum absolute saving. Select and document it before the final comparison. |
| `CLASSIFIER_CACHE_OBSERVATION_TTL_SECS` | Limit how long recent usage is considered. This does not set the provider's cache lifetime. |
| `EVAL_SET` | Name the input test file. |
| `OUT` | Name the output prediction file. |

Read classifier settings in `config.rs` and pass the values to the library's classification code. Keep `EVAL_SET` and `OUT` reads in the example because those settings belong to the evaluation command.

The repository currently reads gateway settings from a library-owned `config.rs`. The brief says classifier settings belong in the binary's configuration. Resolve this placement during Step 5 and document the existing hosting arrangement. Do not introduce environment reads into the model implementation.

Run from the repository root, `/home/chaitanya/projects/nasiko/nasiko_hackthon`.

Download the public sample.

```bash
curl -fsSL https://registry.nasiko.dev/r/nasiko/classifier-eval \
  -o /tmp/classifier-eval.json
```

Run the required command with the classifier settings loaded into the process.

```bash
EVAL_SET=/tmp/classifier-eval.json OUT=/tmp/classifier-out.jsonl \
cargo run --release -p nasiko-llm-router --example classifier_eval
```

Keep the required output fields exactly as the brief specifies. This is an illustrative output, not a measured result.

```json
{"id":"pub-01","request_type":"code_generation","complexity":1,"confidence":0.92,"latency_us":840}
```

Do not write `results.json`. Keep one prediction line per case and put our own aggregate measurements in a separate report.

## Appendix C. Record unresolved questions

| Question | Required evidence |
| --- | --- |
| Are Jev choices and reported confidence repeatable across fresh runs? | Repeated real calls with fixed input and recorded model version. The provider warns that probabilities can vary. |
| Can the organisers reach the Decisions address and authenticate? | Documented address and confirmation that their allowed network path supports it. |
| Does real router context reach Jev? | A routing test where changing context changes the expected interpretation, plus a real router run. |
| Does confidence match observed correctness? | Results grouped by confidence range on untouched checking data. |
| Does the complete command finish in time? | A timed complete CPU run, not only per-call timings. |
| Can the labelled data be reviewed within two hours? | Completed labels and a checked split. |
| Can user turns be linked without mixing conversations? | Verified existing conversation identity, separation tests, and a real multi-turn run. |
| Can recent cache observations predict the next request? | Stable prompt checks, observation age, provider details, and comparison with actual reported usage. |
| Are prices reliable enough to permit switching? | Recorded price source and coverage for both models, including cache prices. |
| Does the classifier improve routing quality or cost? | Matched real conversations with answer-quality checks and costs including Jev. |

No authenticated call has been made as part of this planning task. The saved key has not been validated.

## Appendix D. Read the evidence behind the plan

- [Common submission and judging rules](/home/chaitanya/projects/nasiko/hackathon-problems.md:3).
- [P2 problem and public sample](/home/chaitanya/projects/nasiko/hackathon-problems.md:176).
- [Required evaluation command and output](/home/chaitanya/projects/nasiko/hackathon-problems.md:191).
- [Required classifier, data, routing, and repeatability work](/home/chaitanya/projects/nasiko/hackathon-problems.md:204).
- [Required measurements and acceptance checks](/home/chaitanya/projects/nasiko/hackathon-problems.md:222).
- [Demo checklist](/home/chaitanya/projects/nasiko/hackathon-problems.md:239).
- [Existing token records and calculated request cost](/home/chaitanya/projects/nasiko/nasiko_hackthon/llm-router/src/usage.rs:166).
- [Existing background writes and possible logging failures](/home/chaitanya/projects/nasiko/nasiko_hackthon/llm-router/src/usage.rs:74).
- [Existing pricing engine](/home/chaitanya/projects/nasiko/nasiko_hackthon/pricing/src/engine.rs:121).
- [Price sources and cache prices](/home/chaitanya/projects/nasiko/nasiko_hackthon/pricing/src/quote.rs:44).
- [Current per-turn coding-agent routing identity](/home/chaitanya/projects/nasiko/nasiko_hackthon/llm-router/src/routing/boundary.rs:174).
- [Official public sample and difficulty rules](https://registry.nasiko.dev/r/nasiko/classifier-eval).
- [OpenRouter Jev documentation](https://openrouter.ai/docs/guides/community/jev).
- [OpenRouter Jev tutorial, including confidence and repeated-run differences](https://openrouter.ai/docs/guides/community/jev-tutorial).
- [OpenRouter classification guide](https://openrouter.ai/docs/cookbook/evaluate-and-optimize/jev-classification).
- [OpenRouter Decisions API reference](https://openrouter.ai/docs/api/api-reference/alphadecisions/submit-a-decisions-questions-and-answers-request).

The plan applies Poteto Mode's Sequence Work into Verifiable Units principle by ending each step with evidence. It applies Prove It Works by requiring real router and model runs before claiming completion. Bro and technical-writing guidance keep the instructions in plain English.
