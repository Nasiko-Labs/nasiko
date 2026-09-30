#!/usr/bin/env node
/**
 * Does the generator write DSL this client can actually render?
 *
 * The 209 unit tests prove the parser, materializer, renderer and reactivity
 * are correct. None of them says anything about whether the *model* produces
 * good DSL — so until now the only feedback loop on a prompt change or a
 * catalog change was opening a browser and forming an impression. That is not
 * a feedback loop you can iterate on, and it means a regression is found by
 * whoever hits it.
 *
 * This runs real prompts through the real endpoint and puts the real pipeline
 * behind them. It asserts nothing about *taste* — whether a bar chart was the
 * right call is not checkable — only about the things that are objectively
 * broken:
 *
 *   - the DSL parses, and every statement in it parses
 *   - materializing produces no diagnostics
 *   - nothing references a name that does not exist
 *   - every Query names a source the scope actually allows
 *   - there is a root, and it resolves to a component
 *   - the tree renders without a single component being dropped
 *
 * Every one of those has been a real bug in this pipeline at least once.
 *
 * Usage:
 *   node ui/scripts/eval-generations.mjs                 # against the endpoint
 *   node ui/scripts/eval-generations.mjs --record        # save responses as fixtures
 *   node ui/scripts/eval-generations.mjs --offline       # replay saved fixtures, no network
 *   node ui/scripts/eval-generations.mjs --record --repair   # ...and measure the repair turn
 *   node ui/scripts/eval-generations.mjs --case spend    # one case
 *
 * `--offline` is what CI runs: recorded generations, replayed, so a change to
 * the runtime or the catalog is checked against real model output on every
 * commit without spending a token or depending on a model being up. Re-record
 * deliberately when the prompt changes — the diff in `fixtures/` is then the
 * review artifact showing what the model started writing differently.
 */

import { readFileSync, writeFileSync, existsSync, mkdirSync, readdirSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { resolve, dirname, basename } from 'node:path';
import { fileURLToPath } from 'node:url';

const UI = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const FIXTURES = resolve(UI, 'tests/fixtures/generations');

/**
 * Where every recorded run is kept, one directory per run.
 *
 * `--record` overwrites the fixtures in place, and used to keep no history at
 * all: three baseline runs were taken in one afternoon to measure how much a
 * generation varies with no intervention, and the first two were gone before
 * anyone could compare them. Only the numbers someone had transcribed by hand
 * survived, and those turned out to be component INSTANCE counts rather than
 * vocabulary breadth — so they could not answer the question they were
 * gathered for.
 *
 * A run is cheap to keep (twelve small text files) and impossible to
 * reconstruct, so every one is kept now. Git-ignored: these are experiment
 * output, not source, and they accumulate. To keep one as a durable baseline,
 * `git add -f` it or copy it somewhere outside the repo — `--compare` takes
 * any directory of `.dsl` files.
 */
const RUNS = resolve(UI, 'tests/fixtures/runs');
const CATALOG = resolve(UI, 'common/surface/dsl-catalog.json');

const { parseBuffer } = await import(new URL('../common/surface/parser.js', import.meta.url).href);
const { materialize, buildComponentIndex } = await import(new URL('../common/surface/materialize.js', import.meta.url).href);
const { repairableDiagnostics, buildRepairPrompt } = await import(new URL('../common/surface/repair.js', import.meta.url).href);
const { render } = await import(new URL('../common/surface/render.js', import.meta.url).href);

const catalog = JSON.parse(readFileSync(CATALOG, 'utf8'));
const index = buildComponentIndex(catalog);

/**
 * The prompts.
 *
 * Chosen to exercise the grammar rather than to flatter it: each names a shape
 * the model has to get structurally right, and several are phrased the way a
 * real person asks rather than the way the examples in agent.yaml are written.
 *
 * `expect` is deliberately thin. Asserting a specific component would make
 * this a test of the model's taste, which changes with every prompt tweak and
 * is not a regression when it does. Asserting *at least one Query* or *a table
 * somewhere* catches the failure that matters: a generation that looks fine
 * and contains no real data.
 */
export const CASES = [
  { id: 'spend-14d', prompt: 'Show me spend and request volume for the last 14 days',
    expect: { minQueries: 1, minComponents: 3 } },
  { id: 'agents-table', prompt: 'Which agents cost the most? Table, with a cost breakdown',
    expect: { minQueries: 1, tags: ['app-table'] } },
  { id: 'switchable', prompt: 'Give me a cost dashboard I can switch between cost and operations',
    expect: { minQueries: 1, minActions: 1, minStates: 1 } },
  // The one case that names a component, and it earns it: the prompt says
  // "with a chart", so a tree without one did not answer the question. It has
  // already caught the failure it was written for — a generation that wrote
  // two AppCharts and left the row holding them unreferenced, which the
  // orphaned_statement diagnostic now names directly.
  // The composition case, and the one that has earned its keep. Every recording
  // of this prompt through three catalogs tried to nest a chart inside a card
  // and failed a different way — the chart into `name`, nineteen arguments at a
  // fifteen-argument card, the card and the chart as unconnected statements
  // with both orphaned. None of those was a model that could not compose; it
  // was a model reaching for a shape the grammar had no room for. app-card
  // leads with children now and this passes on the first recording after.
  { id: 'by-model-chart', prompt: 'Usage by model, with a chart',
    expect: { minQueries: 1, tags: ['app-chart'] } },
  // The case this suite did not have, and the gap that let a real complaint go
  // unmeasured. Every other prompt here names ONE thing — a table, a chart, a
  // toggle — so a model that reaches for the same shape every time still
  // passes them all. The dashboard that prompted the component and catalog
  // work was this request, and what was wrong with it was breadth: two line
  // charts side by side, answering a time question and a volume question with
  // the same mark, and nothing else.
  //
  // `minChartKinds: 2` is the assertion, and it is a floor rather than a
  // preference for any particular chart. Line for the trend and anything else
  // for the breakdown both pass; two lines do not. Both tokenops breakdown
  // sources (by agent, by model) are in scope, so the data for a second shape
  // is there to be asked for — this is not a case that can only be satisfied
  // by inventing something.
  //
  // Failed on arrival and stayed failing through four attempts to fix it, all
  // of them edits to what the prompt SAID. It passes now because of the fifth,
  // which changed what the prompt SHOWS — see WORKED EXAMPLE 2b-ii. The two
  // findings, in the order they were established:
  //
  //   1. Chart shapes follow DATA shapes. The first recordings fetched one
  //      source and drew its one row set twice, because they had nothing else
  //      in hand. Declaring what each source ANSWERS — and, for a composite,
  //      what it does NOT contain — is what got a second Query written.
  //   2. Demonstrated code beats described intent. With a series and a
  //      breakdown both fetched, the breakdown still went into a table: every
  //      worked example that built from one did that, and a source line saying
  //      "a bar, donut or ranking" lost to four examples showing otherwise.
  //
  // Keep both assertions. They are a floor, not a style: minQueries catches a
  // dashboard with nothing to be broad about, minChartKinds catches one chart
  // drawn twice. Neither prefers a particular chart.
  { id: 'comprehensive', prompt: 'create a comprehensive tokenops dashboard with charts',
    expect: { minQueries: 2, minComponents: 6, minChartKinds: 2 } },
  { id: 'kpis-only', prompt: 'Just the headline numbers, nothing else',
    expect: { minQueries: 1 } },
  { id: 'filter-days', prompt: 'History chart with buttons to switch between 7 and 30 days',
    expect: { minQueries: 1, minActions: 1, minStates: 1 } },
  { id: 'vague', prompt: 'how are we doing on cost',
    expect: { minQueries: 1 },
  },
  { id: 'terse', prompt: 'spend',
    expect: { minQueries: 1 } },
  // Interactive controls are where a model leaves things unnamed. Three rounds
  // of prompt edits could not make it fill AppSearch's aria-label when it had
  // already written a placeholder saying the same thing, so app-search now
  // names itself from the placeholder and the catalog says so. What this case
  // still catches is a control with neither.
  { id: 'interactive-controls', prompt: 'Cost dashboard with a search box and buttons to change the window',
    expect: { minQueries: 1, minActions: 1 } },
  // The composition-SHAPE case, and the only one here that asks for an
  // arrangement rather than for a component. Every other prompt names one
  // thing — a table, a chart, a toggle — so a surface that drops its controls
  // into a loose row passes all of them, and app-toolbar and app-field went
  // unused across three separate recordings without a single case being able
  // to notice. A worked example demonstrating them changed nothing measurable
  // for exactly that reason: the corpus had no task that needed one.
  //
  // "Group the filters above the table" is the requirement and the whole
  // reason this case exists. Three named filters is more control surface than
  // a bare row carries, and a labelled group sitting above a result set is the
  // shape app-toolbar and app-field are for.
  //
  // What is asserted is deliberately NOT app-toolbar or app-field. This case
  // was added to MEASURE whether demonstration moves that choice, and gating
  // on the outcome would make the question unfalsifiable — it would pass only
  // when the answer was already yes. The floor is what the prompt itself
  // states: real data, a table as the result, and filters that are actually
  // filters rather than decoration (more than one $state, written by more
  // than one Action). How they are grouped is measured, not required.
  //
  // The prompt asked for a MODEL filter until 17 September, and no generation
  // could satisfy it: AgentFinopsRow carries agent_name and no model, and the
  // `model` argument matches exactly rather than searching. Eight recorded
  // runs were scored against a task with no correct answer, three of them
  // inventing @Filter(rows, "model", …) on a field that is not there. The
  // case now asks only for filters the fleet source can actually express —
  // one server-side (range) and one client-side (agent_name), which is still
  // both halves of the mechanism decision.
  { id: 'grouped-filters',
    prompt: 'Show TokenOps usage in a table, filterable by agent and by date range. '
      + 'Group the filters above the table.',
    // The one annotation that survived the contract/quality split, and it is
    // the one that should: this recording references `rowsQ`, a statement it
    // never defines, so the surface does not resolve. That is a CONTRACT
    // failure — the fixture is not a working surface, which makes it useless
    // as a regression fixture, whatever it says about the generator.
    //
    // So this is not "the rate is high, look away". It is "this particular
    // draw is not fit to be the sample", and the fix is a re-draw, not an
    // annotation. It stays only until one is taken. The underlying defect —
    // the generator losing track of its own statement names on a long
    // surface, ~42% (10/24 at b663b6eaa08a) — is separate and open.
    knownFailure: 'the recorded surface references "rowsQ", which it never defines, so it does '
      + 'not resolve. Re-draw it; this annotation is a placeholder, not a verdict. See NAS-755.',
    expect: { minQueries: 1, minActions: 2, minStates: 2, tags: ['app-table'] } },
  // The other half of rule 20, and the half `grouped-filters` cannot reach.
  // Every state in that case turned out to be a real argument of
  // fetchTokenopsDashboard, so "a state in the args needs an @Run" was
  // exercised three times over and "a state that is NOT an argument must not
  // have one" was never exercised at all — the rule's negative clause has
  // never been tested against a generation.
  //
  // fetchUsageHistory is the one source that forces it. Its whole argument
  // list is `days`, and its rows come back bare, so a box that narrows those
  // rows has nowhere server-side to go: the only way to satisfy the request
  // is @Filter over what was already fetched, and the Action that sets the
  // search must not re-run a Query whose one argument did not change.
  //
  // The prompt names the task, not the mechanism — no "client-side", no
  // "already fetched". A model that reads the source line has everything it
  // needs to work the strategy out, and one that does not will reach for the
  // shape it saw last. That difference is the measurement.
  //
  // Assertions are the same floor as grouped-filters and deliberately do not
  // mention @Filter or @Run: redundantRuns() already fails the wiring defect
  // objectively, and asserting the mechanism here would gate on the answer.
  { id: 'client-side-filter',
    prompt: 'Daily usage history as a table, with a control to switch between 7, 30 and 90 days '
      + 'and a search box to narrow the rows.',
    expect: { minQueries: 1, minActions: 2, minStates: 2, tags: ['app-table'] } },
  // The positive control for semantic_control_argument_mismatch, and the one
  // reading the analyzer does not have.
  //
  // It flags free text into an exactly-matched argument, which is right when
  // the box says "Search agents..." and the argument is agentId — a partial
  // name returns nothing. But a free-text box into an exact argument is not
  // wrong in itself: pasting an id you already have is a real interaction, and
  // an exact argument is exactly what it should reach. Three true positives
  // across seven runs prove the check fires; none of them prove it can stay
  // silent when free text is the right answer, because no case has ever asked
  // for one.
  //
  // So the prompt asks for identifier ENTRY, in those words, and names no
  // component. What is being measured is whether the model reaches for a
  // free-text control here at all, and if it does, whether the analyzer's rule
  // as written calls it a mismatch — which would make it a false positive on
  // its own terms.
  //
  // Floor only, and no tag assertion: "its usage" could honestly be a stat
  // row, a table or a card, and pinning one would measure obedience rather
  // than wiring.
  { id: 'agent-by-id',
    prompt: 'Look up one agent by its exact agent ID and show its usage. '
      + 'The ID is typed or pasted in.',
    expect: { minQueries: 1, minActions: 1, minStates: 1 } },
  // Candidate A. The first case whose source is server-paged and
  // server-searchable, and the first where the RIGHT answer is the opposite
  // of what worked example 3f demonstrates.
  //
  // 3f teaches that a search box is a client-side @Filter, because
  // `agent_name` is a field on the fleet rows and not an argument of the
  // source. On fetchUsageByAgent, `query` IS an argument — the manifest says
  // `text [search]` — so the same request has the opposite answer, and a
  // generation that reaches for @Filter here has learned the shape of the
  // example rather than the procedure in it. Nothing else in this suite can
  // tell those two apart.
  //
  // Four decisions, and the corpus says the model currently makes at most one
  // of them: across every recorded generation, `page` and `limit` have only
  // ever been written as frozen literals (`["", 1, 50]`, by-model-chart, four
  // times out of four), `total` has never been read, and 54 of 56 app-tables
  // page client-side over whatever one fetch returned.
  //
  // `expect` is the same thin floor as the other interaction cases and says
  // nothing about pagination, search or total. Those four are recorded beside
  // it by `expect.mechanism`, separately and unweighted — see pagedMechanism.
  { id: 'paged-agent-usage',
    prompt: 'My own usage broken down by agent, 20 rows at a time with next and previous '
      + 'buttons, and a box to search agents by name. Show how many there are in total.',
    // Fails ~62% of live runs (15/24 at b663b6eaa08a) — a state set and never
    // @Run, a source that cannot answer the request, a @Filter on a field the
    // source does not return. Four mechanisms in one prompt and the generator
    // gets three on a good day. It carried a knownFailure for one afternoon;
    // once contract and quality were counted apart the recording turned out
    // to RENDER fine, so the annotation was stale and the harness said so.
    // The rate is still real and still reported — as quality, where a number
    // that varies belongs.
    expect: {
      minQueries: 1, minStates: 2, tags: ['app-table'],
      mechanism: { source: 'fetchUsageByAgent' },
    } },
  // The control-kind pair. One source, one argument, one required refetch,
  // and the ONLY difference between the two prompts is which control the
  // reader is offered.
  //
  // Everything measured so far says `@Run` production tracks the control
  // kind rather than the source's argument list — app-search 10/32 against
  // 40/44 for every other kind, and inside grouped-filters 8/18 against
  // 20/20, sometimes in one surface. Every one of those numbers is
  // observational: control kind has never been varied with the source and
  // the argument held still. These two cases do that and nothing else.
  //
  // fetchSpendCalendar is the source that makes it clean. `month` is
  // REQUIRED and is a plain string sent verbatim, so typing "2026-08" and
  // picking "2026-08" are byte-identical requests; and the response carries
  // only the month that was asked for, so there is no client-side mechanism
  // to choose instead. The mechanism decision that tangled every earlier
  // experiment is absent, and what is left is whether the refetch gets
  // wired. The source has also never been generated once in the whole
  // corpus and appears in no worked example, so neither arm has a
  // same-source template to copy.
  //
  // `expect` is a floor and says nothing about the control or the @Run. The
  // four readings are recorded by `controlPair` and never gate, because a
  // case that asserts the answer it measures can only return the answer it
  // was given.
  //
  // Accessible naming is `aria-label` in both arms on purpose: app-search
  // has no `label` parameter (nameFrom: aria-label, placeholder) while
  // app-segmented-control does (nameFrom: label, aria-label), and a request
  // implying a visible label would push the choice toward one arm. That
  // asymmetry is what contaminated agent-by-id.
  { id: 'month-typed',
    prompt: 'Daily spend for one calendar month, as a bar chart. '
      + 'Let me type the month I want as YYYY-MM.',
    expect: {
      minQueries: 1, minActions: 1, minStates: 1,
      controlPair: { source: 'fetchSpendCalendar', argument: 'month', control: 'app-search' },
    } },
  { id: 'month-picked',
    prompt: 'Daily spend for one calendar month, as a bar chart. '
      + 'Let me pick between July, August and September 2026.',
    expect: {
      minQueries: 1, minActions: 1, minStates: 1,
      controlPair: { source: 'fetchSpendCalendar', argument: 'month', control: 'app-segmented-control' },
    } },
  // The control-kind pair, second attempt. The first one never reached the
  // question: "let me type the month as YYYY-MM" produced app-date-field 6/12
  // and app-input 6/12 and app-search 0/12, so there was no app-search arm to
  // compare. That pair is closed as exploratory and stays in the corpus.
  //
  // What it taught is what selects a control, measured over 101 recorded
  // generations: every prompt containing "search box" or "search ... by name"
  // produced app-search — 2/2, 4/4, 22/22 — and every prompt saying "type"
  // produced something else. So arm A says "search box" and "by name", and
  // says nothing about typing, entering, pasting, identifiers or formats.
  //
  // fetchSpendTimeseries.model is the argument that makes the pair clean. It
  // is a plain exact-match string with no format, no calendar and no
  // identifier convention, so no specialised control is in reach: a model
  // NAME is what app-search is for, and a named set of three is what a
  // segmented control is for. The source appears in no worked example at all
  // (zero mentions in agent.yaml) and its handful of incidental generations
  // never bound a control to it, so neither arm has a same-source template.
  //
  // Everything else is held still: same source, same `model` argument, same
  // `range: "7d"` literal, same series, same line chart, same heading, same
  // single $model state, and aria-label as the accessible name in both —
  // app-search has no `label` parameter and a request implying a visible one
  // would push the choice toward the other arm.
  //
  // `expect` stays a floor. The four readings live in `controlPair` and never
  // gate, because a case that asserts the answer it measures can only return
  // the answer it was given.
  { id: 'model-searched',
    prompt: 'Spend over the last 7 days for one model. '
      + 'Use a search box to find the model by name.',
    // The app-search arm of the control pair, ~56% (15/27 at b663b6eaa08a).
    // Low @Run production on app-search is the FINDING this pair exists to
    // observe, so it was never a thing to gate on — and now it is not one:
    // the missing @Run is a quality reading, printed every replay, counted on
    // every live run, and it stops no build. `model-picked` is the same
    // measurement with the other affordance, and the contrast is the result.
    expect: {
      minQueries: 1, minActions: 1, minStates: 1,
      controlPair: { source: 'fetchSpendTimeseries', argument: 'model', control: 'app-search' },
    } },
  { id: 'model-picked',
    prompt: 'Spend over the last 7 days for one model. Use a segmented control to choose '
      + 'between claude-sonnet-4-5, claude-haiku-4-5, and claude-opus-4-1.',
    expect: {
      minQueries: 1, minActions: 1, minStates: 1,
      controlPair: { source: 'fetchSpendTimeseries', argument: 'model', control: 'app-segmented-control' },
    } },
  // Isolated pair. Same two prompts, byte for byte, against a source
  // inventory with no free-text parameter in it — `tokenops_isolated` drops
  // fetchUsageByAgent and fetchUsageByModel, the only two sources carrying a
  // `query [search]` argument. Run these ONLY under _DASHBOARD_SCOPE=
  // tokenops_isolated; under the normal scope they are duplicates of the pair
  // above and measure nothing.
  { id: 'model-searched-isolated',
    prompt: 'Spend over the last 7 days for one model. '
      + 'Use a search box to find the model by name.',
    expect: {
      minQueries: 1, minActions: 1, minStates: 1,
      controlPair: { source: 'fetchSpendTimeseries', argument: 'model', control: 'app-search' },
    } },
  { id: 'model-picked-isolated',
    prompt: 'Spend over the last 7 days for one model. Use a segmented control to choose '
      + 'between claude-sonnet-4-5, claude-haiku-4-5, and claude-opus-4-1.',
    expect: {
      minQueries: 1, minActions: 1, minStates: 1,
      controlPair: { source: 'fetchSpendTimeseries', argument: 'model', control: 'app-segmented-control' },
    } },
  // EXPLORATORY, not a clean test of result shape. The catalog has no second
  // independent row-shaped source with an exact `model` argument, so this
  // answers one question only: does the search-vs-picker gap persist when both
  // controls drive the same breakdown source? Run ONLY under
  // _DASHBOARD_SCOPE=tokenops_rows, which drops fetchTokenopsDashboard — it
  // carries the identical rows, so leaving it in makes "which source" moot.
  { id: 'model-rows-searched',
    prompt: 'Cost per agent for one model over the last 7 days, as a table. '
      + 'Use a search box to find the model by name.',
    expect: {
      minQueries: 1, minActions: 1, minStates: 1,
      controlPair: { source: 'fetchFinopsAttributions', argument: 'model', control: 'app-search' },
    } },
  { id: 'model-rows-picked',
    prompt: 'Cost per agent for one model over the last 7 days, as a table. '
      + 'Use a segmented control to choose between claude-sonnet-4-5, claude-haiku-4-5, '
      + 'and claude-opus-4-1.',
    expect: {
      minQueries: 1, minActions: 1, minStates: 1,
      controlPair: { source: 'fetchFinopsAttributions', argument: 'model', control: 'app-segmented-control' },
    } },
  // Not a dashboard request. agent.yaml rule 11 says answer in plain text, so
  // the correct outcome is prose and *no* DSL — a generator that builds a
  // dashboard here is broken in a way no other case would catch.
  { id: 'greeting', prompt: 'hey, what can you do?',
    expect: { noSurface: true } },
  // Names a source that is not in the tokenops scope. The line filter should
  // drop it, so the correct outcome is either no Query or a refusal — never a
  // Query naming something that does not exist.
  { id: 'out-of-scope', prompt: 'Show me our AWS bill by service',
    expect: { allowNoSurface: true } },
];

/**
 * How seriously to take each diagnostic — read from the generated manifest, not
 * hand-kept here.
 *
 * This was a three-entry literal, and two of the three were added *reactively*,
 * after a new code turned a correct surface red. That is the wrong direction to
 * fail in: it teaches people to ignore the checker. `gen-diagnostics.mjs` now
 * requires a decision per code and `--check` refuses an unclassified one, so
 * the next code cannot arrive fatal-by-omission.
 *
 *   fatal     the generation is wrong — something asked for is missing or wrong
 *   advisory  the runtime corrected a real mistake; the surface is still right
 *   runtime   a failed source, a dropped stream, a host gap. Says nothing about
 *             the DSL, and should not appear offline at all — if one does, the
 *             harness is what is broken, so it is printed under its own heading
 *             rather than folded in with the model's mistakes.
 */
const SEVERITY = JSON.parse(
  readFileSync(new URL('../common/surface/diagnostics.json', import.meta.url), 'utf8'),
).diagnostics;

/** Unknown means unclassified means fatal — the gate should have caught it. */
const severityOf = (code) => SEVERITY[code]?.severity ?? 'fatal';

/** How many components the catalog offers — the denominator for breadth. */
const CATALOG_SIZE = Object.keys(catalog.components ?? {}).length;

/**
 * The component kinds a generation actually rendered.
 *
 * Off `evaluateGeneration`'s `tags`, which is a walk of the MATERIALIZED tree,
 * never a regex over the DSL text. A regex counts named statements and misses
 * a component written inline inside another call — `AppCard([AppStatCard(…)])`
 * is one statement and two components. Measuring breadth that way undercounted
 * a real corpus by one kind and produced a confident wrong baseline, so the
 * rule is: the evaluator is the only thing that counts components.
 */
const kindsOf = (r) => [...new Set(r.tags ?? [])].sort();

/** `{ code: n }` for one generation's diagnostics, and how many were fatal. */
function diagnosticTally(r) {
  const counts = {};
  let fatal = 0;
  for (const d of r.diagnostics ?? []) {
    counts[d.code] = (counts[d.code] ?? 0) + 1;
    if (severityOf(d.code) === 'fatal') fatal++;
  }
  return { counts, fatal };
}

const sha256 = (text) => createHash('sha256').update(text).digest('hex').slice(0, 16);

/** Evaluate a directory of `.dsl` files the same way a live run is evaluated. */
/** A saved run's `generator` block, when it has one. */
function readProvenance(dir) {
  try {
    const s = JSON.parse(readFileSync(resolve(dir, 'summary.json'), 'utf8'));
    return s.generator ? { ...s.generator, consistent: s.generatorConsistent } : null;
  } catch { return null; }
}

function evaluateDir(dir) {
  const out = {};
  for (const f of readdirSync(dir).filter((x) => x.endsWith('.dsl')).sort()) {
    const text = readFileSync(resolve(dir, f), 'utf8');
    const r = evaluateGeneration(text);
    const { counts, fatal } = diagnosticTally(r);
    out[f.replace(/\.dsl$/, '')] = {
      kinds: kindsOf(r), instances: (r.tags ?? []).length, diagnostics: counts, fatal,
    };
  }
  return out;
}

/**
 * Every source the scope allows. Anything else must not survive to the client.
 *
 * Read from the generated manifest rather than kept by hand. The hand-kept
 * version listed five and went stale the day NAS-582 widened the scope to
 * seven, so a generation that correctly used fetchSpendTimeseries — a source
 * the manifest offers and the model was told about — failed the eval for
 * naming it. A checker that disagrees with the contract it is checking is
 * worse than no checker: it teaches you to distrust a red.
 */
const MANIFEST = JSON.parse(
  readFileSync(new URL('../common/surface/data-manifest.json', import.meta.url), 'utf8'),
);
export const ALLOWED_SOURCES = new Set(
  Object.values(MANIFEST.scopes ?? {}).flat().map((s) => (typeof s === 'string' ? s : s.name)),
);

/**
 * The closed sets each source's arguments accept, off the same manifest.
 *
 * `argsEnum` is derived by gen-data-manifest from the argsShape prose the
 * model is shown, so this checks the generation against the very text it was
 * given — not against a second opinion. The runtime now refuses one of these
 * before it reaches the network (`arg_enum_violation` in queries.js), but the
 * eval never runs the query manager: it materializes and reads. So the same
 * fact has to be asserted here, statically, or the corpus stays blind to a
 * defect the browser already catches.
 */
const ARG_ENUMS = new Map(
  Object.values(MANIFEST.scopes ?? {}).flat()
    .filter((s) => typeof s !== 'string' && s.argsEnum)
    .map((s) => [s.name, {
      callStyle: s.callStyle,
      keys: Object.keys(s.argsShape ?? {}),
      enums: s.argsEnum,
    }]),
);

/**
 * Arguments a source would reject, for one Query declaration.
 *
 * The value as materialized, which is the value the fetch would carry. A
 * `$state` reference has already been evaluated to its declared default by
 * the time a declaration exists, so "what the model literally typed" is not
 * a distinction available here — and in a replay there is no user, so every
 * value on this path is the model's anyway.
 *
 * Blind to an option a control offers but has not selected: only the current
 * value is materialized. A picker with one bad choice in three reads clean.
 */
function argumentsOutsideEnum(query) {
  const spec = ARG_ENUMS.get(query.source);
  if (!spec) return [];
  const args = query.args ?? [];
  const valueOf = spec.callStyle === 'object'
    ? (name) => (args[0] && typeof args[0] === 'object' ? args[0][name] : undefined)
    : (name) => args[spec.keys.indexOf(name)];
  const out = [];
  for (const [name, allowed] of Object.entries(spec.enums)) {
    const value = valueOf(name);
    if (value === undefined || value === null || value === '') continue;
    if (typeof value !== 'string' && typeof value !== 'number') continue;
    if (allowed.includes(String(value))) continue;
    out.push({ source: query.source, argument: name, value, allowed });
  }
  return out;
}

/**
 * One run's provenance, and whether it is one run at all.
 *
 * A recording is an experimental condition only if every case in it came from
 * the same generator. The last A/B could not prove that: 3f was committed
 * thirteen minutes before the first "baseline" case, the arms were separated
 * by a redeploy nobody recorded, and the only trace of it was a network error
 * in the middle. A run whose cases disagree is not a result to be argued
 * about later — it is two half-runs, and saying so at record time costs
 * nothing.
 */
export function runProvenance(byCase) {
  const seen = Object.values(byCase).filter(Boolean);
  if (!seen.length) return { generator: null, generatorConsistent: true, generatorSpread: null };
  const digests = [...new Set(seen.map((g) => g.promptDigest ?? 'unreported'))];
  // Prompt identity alone misses boundary, inventory and model changes.
  const conditions = new Set(seen.map(g => JSON.stringify(provenanceFrom(g))));
  const consistent = conditions.size === 1;
  const spread = consistent ? null : Object.fromEntries(
    digests.map((d) => [d, Object.keys(byCase).filter((id) => (byCase[id]?.promptDigest ?? 'unreported') === d)]),
  );
  return { generator: seen[0], generatorConsistent: consistent, generatorSpread: spread };
}

/**
 * What a recorded run has to say about the generator that produced it.
 *
 * Every earlier run recorded `catalogVersion` — which comes from THIS
 * checkout, not from the agent — and a hand-set `manifestVersion` that
 * nothing compares. So a saved surface said nothing about the prompt behind
 * it, and the last A/B could only be attributed by the order the containers
 * were deployed in. That is a story about a deployment, not evidence about a
 * generation, and it is why that experiment is inconclusive rather than
 * negative.
 *
 * `promptDigest` is taken over the ASSEMBLED system message on the generator
 * side, not over agent.yaml: the message is built from the spec's
 * instructions, the component signatures, the data-source signatures and the
 * builtin list, and two prompt changes that moved generation measurably never
 * touched agent.yaml at all. `specDigest` answers the narrower "which
 * agent.yaml" question beside it, `generatorDigest` covers the code that
 * assembles the prompt and filters the output, and `model` changes what comes
 * back without moving any of the three.
 */
const PROVENANCE_KEYS = ['promptDigest', 'specDigest', 'generatorDigest', 'model', 'catalogVersion',
  'scope', 'sourceInventoryDigest', 'sourceNames', 'generationSettings'];

/**
 * The provenance out of one `surface` event's payload.
 *
 * Every key, always, even when the generator did not send it: a run recorded
 * against an older agent has to be visibly unattributed rather than quietly
 * missing the field.
 */
export function provenanceFrom(meta) {
  return Object.fromEntries(PROVENANCE_KEYS.map((k) => [k, meta?.[k] ?? null]));
}

/** A recording element — the same shape the renderer tests use. */
function makeEl(tag) {
  return {
    tag, attrs: {}, children: [], listeners: {},
    setAttribute(k, v) { this.attrs[k] = v; },
    hasAttribute(k) { return Object.prototype.hasOwnProperty.call(this.attrs, k); },
    appendChild(c) { this.children.push(c); return c; },
    addEventListener(t, f) { (this.listeners[t] ||= []).push(f); },
    replaceChildren() { this.children.length = 0; },
  };
}

function walk(el, fn) {
  fn(el);
  for (const c of el.children) walk(c, fn);
}

/**
 * Everything the client would do with this text, and everything it complained
 * about on the way. No network, no DOM — the same modules the browser runs.
 */
/**
 * Split a call's argument list at top level, so `Query("x", [{a: 1, b: 2}], …)`
 * yields three arguments and not five. Bracket-aware and string-aware; returns
 * [] for text that is not a call.
 */
function topLevelArgs(raw, callName) {
  const open = raw.indexOf(`${callName}(`);
  if (open < 0) return [];
  let i = open + callName.length + 1, depth = 0, quote = null, start = i;
  const out = [];
  for (; i < raw.length; i++) {
    const c = raw[i];
    if (quote) { if (c === quote && raw[i - 1] !== '\\') quote = null; continue; }
    if (c === '"' || c === "'") { quote = c; continue; }
    if (c === '(' || c === '[' || c === '{') { depth++; continue; }
    if (c === ')' && depth === 0) { out.push(raw.slice(start, i)); return out; }
    if (c === ')' || c === ']' || c === '}') { depth--; continue; }
    if (c === ',' && depth === 0) { out.push(raw.slice(start, i)); start = i + 1; }
  }
  return out;
}

/**
 * Actions that re-run a Query none of their own state reaches.
 *
 * The narrow, objective half of a wiring mistake the toolbar worked example
 * caused and nothing else caught. A generation filtered agents client-side
 * with `@Filter` — correct for a source with no name argument — but kept the
 * `@Run(dashboardQ)` from the server-side shape it was adapting. The state it
 * sets is not one of that Query's arguments, so the refetch asks for byte-wise
 * identical data on every keystroke: no diagnostic fires, nothing on screen
 * moves, and the surface is wrong only in what it costs.
 *
 * Deliberately narrow, and deliberately not a claim about architecture:
 *
 *   - An Action with no `@Set` at all is left alone. A bare `@Run` is a
 *     Refresh button, which is exactly right.
 *   - An Action is flagged only when NONE of the states it sets appear in the
 *     Query's argument list. One state in the args justifies the `@Run` for
 *     every other set in the same Action.
 *   - Matching is on the ARGUMENT list only, not the default or the dot-path,
 *     because a `$state` in the default says nothing about whether the fetch
 *     would change.
 *
 * Textual rather than structural on purpose: by the time a Query is
 * materialized its arguments hold VALUES, and `{range: "7d"}` no longer
 * remembers that it came from `$range` — which is the one fact this needs.
 */
function redundantRuns(statements) {
  const argsOf = new Map();
  for (const st of statements) {
    const raw = st.raw ?? '';
    const m = /^\s*([A-Za-z_$][\w$]*)\s*=\s*Query\s*\(/.exec(raw);
    if (m) argsOf.set(m[1], topLevelArgs(raw, 'Query')[1] ?? '');
  }
  const found = [];
  for (const st of statements) {
    const raw = st.raw ?? '';
    const named = /^\s*([A-Za-z_$][\w$]*)\s*=\s*Action\s*\(/.exec(raw);
    if (!named) continue;
    const sets = [...raw.matchAll(/@Set\(\s*(\$[\w$]+)/g)].map((x) => x[1]);
    if (!sets.length) continue;
    for (const q of [...raw.matchAll(/@Run\(\s*([A-Za-z_$][\w$]*)\s*\)/g)].map((x) => x[1])) {
      const args = argsOf.get(q);
      if (args === undefined) continue;
      if (sets.some((v) => new RegExp(`\\${v}\\b`).test(args))) continue;
      found.push({ action: named[1], query: q, states: sets });
    }
  }
  return found;
}

/**
 * Every data source by name, so an argument can be looked up by the source
 * that declares it. Same manifest ALLOWED_SOURCES is built from.
 */
const SOURCES_BY_NAME = new Map(
  Object.values(MANIFEST.scopes ?? {}).flat()
    .filter((s) => typeof s !== 'string')
    .map((s) => [s.name, s]),
);

/**
 * What KIND of value a control hands to `$event`, read off the catalog rather
 * than listed here.
 *
 * The distinction that matters is whether the value came from a closed set the
 * component itself defines, or from a person typing. `options`/`items` IS that
 * closed set — a combobox counts as a picker even though you type into it,
 * because what it commits is one of its options. A `value` whose description
 * names a literal format (app-date-field's `YYYY-MM-DD`) is neither: the
 * component guarantees the shape.
 *
 * Anything with no `value` attribute at all — a button, a modal — writes
 * nothing a query argument could read, and is not classified.
 */
function controlValueKind(tag) {
  const def = catalog.components?.[tag];
  if (!def) return null;
  const attrs = def.attributes ?? {};
  if (!attrs.value) return null;
  if (attrs.options || attrs.items) return 'picked';
  const desc = String(attrs.value.description ?? '');
  if (/`[A-Za-z]{2,}-[A-Za-z]{2,}/.test(desc) || /ISO[ -]?8601/i.test(desc)) return 'formatted';
  if (attrs.placeholder) return 'free-text';
  return null;
}

/**
 * What KIND of value a source's argument expects, read off the manifest's own
 * `argsShape` prose — the same sentence the model is shown.
 *
 * Deliberately parsed rather than re-declared. A second hand-kept table of
 * argument types is a table that drifts from the one the generator reads, and
 * then this check would be judging against a contract nobody was given.
 */
function argumentKind(sourceName, argName) {
  const shape = SOURCES_BY_NAME.get(sourceName)?.argsShape;
  const text = shape?.[argName];
  if (typeof text !== 'string') return null;
  if (/"[^"]+"\s*\|\s*"/.test(text)) return 'enum';
  if (/\bnumber\b/i.test(text)) return 'number';
  if (/ISO[ -]?8601/i.test(text) || /"[A-Z]{2,}-[A-Z]{2,}/.test(text)) return 'formatted';
  if (/\bsearch\b/i.test(text)) return 'search';
  if (/\bstring\b/i.test(text)) return 'exact-string';
  return null;
}

/** Positional sources name their arguments by order; object ones by key. */
function argNameAt(sourceName, index) {
  const src = SOURCES_BY_NAME.get(sourceName);
  if (!src || src.callStyle !== 'positional') return null;
  return Object.keys(src.argsShape ?? {})[index] ?? null;
}

/**
 * A control whose value cannot be what the argument it feeds is asking for.
 *
 * ONE class, on purpose: a free-text box wired to an argument the source
 * matches exactly. `AppSearch → agentId` renders, raises no diagnostic, and
 * returns nothing for every partial name anyone types — the table just looks
 * empty, which reads as "no usage" rather than as a wiring mistake. It was
 * generated twice in a row and nothing in the harness could see it.
 *
 * Everything else is left alone even where it looks suspect, because this is
 * the boundary between a defect and a taste assertion:
 *
 *   - free-text into a `search` argument is the argument doing its job
 *   - a picker into anything is a value the component guarantees
 *   - a formatted control (app-date-field) into a formatted argument is right,
 *     and into anything else is a different question than this one
 *   - a state that reaches @Filter rather than a query argument is not a
 *     query argument problem at all
 *   - an argument the manifest does not describe is not judged
 *
 * Advisory, never fatal. It is a claim about what the data will do, not about
 * what rendered, and the manifest prose it reads is written for people.
 */
function semanticMismatches(statements) {
  const lines = statements.map((st) => st.raw ?? '');

  // $state -> the Action that sets it, and that Action -> the control that
  // references it. Textual for the same reason redundantRuns is: a
  // materialized call holds VALUES, and by then nothing remembers which
  // control wrote them.
  const setterOf = new Map();
  for (const l of lines) {
    const act = /^\s*([A-Za-z_$][\w$]*)\s*=\s*Action\s*\(/.exec(l);
    if (!act) continue;
    for (const m of l.matchAll(/@Set\(\s*(\$[\w$]+)/g)) setterOf.set(m[1], act[1]);
  }
  const controlOf = new Map();
  for (const l of lines) {
    const comp = /^\s*[A-Za-z_$][\w$]*\s*=\s*(App[A-Za-z]+)\s*\(/.exec(l);
    if (!comp) continue;
    const tag = pascalToTag(comp[1]);
    for (const act of new Set(setterOf.values())) {
      if (new RegExp(`\\b${act}\\b`).test(l)) controlOf.set(act, tag);
    }
  }

  const found = [];
  for (const l of lines) {
    const q = /^\s*([A-Za-z_$][\w$]*)\s*=\s*Query\s*\(\s*"([^"]+)"/.exec(l);
    if (!q) continue;
    const [queryName, source] = [q[1], q[2]];
    const argsText = topLevelArgs(l, 'Query')[1] ?? '';
    // Every `name: $state` an object-style call passes, and every `$state` a
    // positional one passes, paired with the argument it lands in.
    const pairs = [];
    for (const m of argsText.matchAll(/([A-Za-z_][\w]*)\s*:\s*(\$[\w$]+)/g)) {
      pairs.push({ arg: m[1], state: m[2] });
    }
    if (!pairs.length) {
      const positional = topLevelArgs(`f(${argsText.trim().replace(/^\[|\]$/g, '')})`, 'f');
      positional.forEach((raw, i) => {
        const st = /^\s*(\$[\w$]+)\s*$/.exec(raw);
        const arg = argNameAt(source, i);
        if (st && arg) pairs.push({ arg, state: st[1] });
      });
    }
    for (const { arg, state } of pairs) {
      const tag = controlOf.get(setterOf.get(state));
      if (!tag) continue;
      const valueKind = controlValueKind(tag);
      const argKind = argumentKind(source, arg);
      if (valueKind !== 'free-text' || argKind !== 'exact-string') continue;
      found.push({
        control: tag,
        state,
        query: queryName,
        source,
        argument: arg,
        valueKind,
        argumentKind: argKind,
        reason: `${tag} writes ${state} as free text, and ${source}'s "${arg}" is matched `
          + 'exactly — a partial value returns nothing, with no error to say so',
      });
    }
  }
  return found;
}

/** AppStatCard -> app-stat-card, the one direction the catalog does not store. */
function pascalToTag(name) {
  return name.replace(/([a-z0-9])([A-Z])/g, '$1-$2').toLowerCase();
}

/**
 * A state a Query reads that no Action re-runs it for.
 *
 * The mirror of {@link redundantRuns}, and the half the corpus could not see.
 * `@Run` forces and `$state` does not — a variable changing never re-fetches
 * on its own (queries.js:24, agent.yaml rule 5) — so a filter whose value IS
 * one of a Query's arguments and whose Action never `@Run`s it is inert:
 * typing moves the store, the args the fetch was made under do not change,
 * and the table sits there showing the old data. It renders, nothing is
 * unresolved, no diagnostic fires, and the checker passed one of these while
 * two people looked at it.
 *
 * Together the pair states one invariant in both directions: the set of
 * states an Action sets and the set of states a Query reads have to agree
 * about whether that Action re-runs that Query.
 *
 * Two exclusions, both about not inventing a defect:
 *
 *   - A state no Action sets is a constant with an initial value. There is no
 *     Action to have forgotten anything.
 *   - A surface can deliberately separate typing from submitting: the input
 *     sets the state, and a Search or Refresh button carries the `@Run`. That
 *     button is an Action that runs the Query and sets none of its state
 *     arguments, which is exactly what a bare trigger looks like, so one of
 *     those anywhere in the surface suppresses this for that Query.
 */
function missingQueryRuns(statements) {
  const lines = statements.map((st) => st.raw ?? '');

  /** queryName -> the `$state` names its argument list reads. */
  const stateArgsOf = new Map();
  for (const l of lines) {
    const q = /^\s*([A-Za-z_$][\w$]*)\s*=\s*Query\s*\(/.exec(l);
    if (!q) continue;
    const args = topLevelArgs(l, 'Query')[1] ?? '';
    stateArgsOf.set(q[1], new Set([...args.matchAll(/(\$[\w$]+)/g)].map((m) => m[1])));
  }

  /** Every Action, with what it sets and what it runs. */
  const actions = [];
  for (const l of lines) {
    const a = /^\s*([A-Za-z_$][\w$]*)\s*=\s*Action\s*\(/.exec(l);
    if (!a) continue;
    actions.push({
      name: a[1],
      sets: new Set([...l.matchAll(/@Set\(\s*(\$[\w$]+)/g)].map((m) => m[1])),
      runs: new Set([...l.matchAll(/@Run\(\s*([A-Za-z_$][\w$]*)\s*\)/g)].map((m) => m[1])),
    });
  }

  // A Query with its own submit button needs no per-setter @Run.
  const hasBareTrigger = new Set();
  for (const act of actions) {
    for (const q of act.runs) {
      const stateArgs = stateArgsOf.get(q);
      if (!stateArgs) continue;
      if (![...act.sets].some((v) => stateArgs.has(v))) hasBareTrigger.add(q);
    }
  }

  const found = [];
  for (const [query, stateArgs] of stateArgsOf) {
    if (hasBareTrigger.has(query)) continue;
    for (const state of stateArgs) {
      const setters = actions.filter((act) => act.sets.has(state));
      if (!setters.length) continue;
      for (const act of setters) {
        if (act.runs.has(query)) continue;
        found.push({ action: act.name, query, state });
      }
    }
  }
  return found;
}

/** Every field name anywhere in a source's declared response, at any depth. */
function fieldsOf(sourceName) {
  const seen = new Set();
  (function walk(node) {
    if (Array.isArray(node)) { node.forEach(walk); return; }
    if (node && typeof node === 'object') {
      for (const [key, value] of Object.entries(node)) {
        if (!key.startsWith('$')) seen.add(key);
        walk(value);
      }
    }
  })(SOURCES_BY_NAME.get(sourceName)?.responseShape);
  return seen;
}

/**
 * An `@Filter` naming a field the source does not return.
 *
 * `@Filter(rows, "model", "contains", $model)` against the fleet dashboard
 * matches nothing, every time: AgentFinopsRow carries agent_name and no model
 * at all. The table renders empty, which reads as "no usage in this period"
 * rather than as a filter pointed at a field that does not exist, and no
 * layer says otherwise — the renderer checks the DSL against the catalog and
 * never against the response.
 *
 * Three of the twelve A/B generations did this, two of them after being shown
 * the field list.
 *
 * Only field validity, nothing about intent. The chain is followed back to
 * the Query it started from — `@Filter(@Filter(q, …), …)` is two links — and
 * a first argument that does not resolve to a Query is left alone rather than
 * guessed at. Read off the same generated manifest ALLOWED_SOURCES comes
 * from, so the fields checked are the fields the model was shown.
 */
function unknownFilterFields(statements) {
  const lines = statements.map((st) => st.raw ?? '');

  /** statement name -> the source its value ultimately comes from. */
  const sourceOfName = new Map();
  for (const l of lines) {
    const q = /^\s*([A-Za-z_$][\w$]*)\s*=\s*Query\s*\(\s*"([^"]+)"/.exec(l);
    if (q) sourceOfName.set(q[1], q[2]);
  }
  // A @Filter over a @Filter inherits the source. Repeat until nothing new
  // resolves, so the order statements appear in does not matter.
  for (let pass = 0; pass < lines.length; pass++) {
    let grew = false;
    for (const l of lines) {
      const f = /^\s*([A-Za-z_$][\w$]*)\s*=\s*@Filter\(\s*([A-Za-z_$][\w$]*)/.exec(l);
      if (!f || sourceOfName.has(f[1])) continue;
      const from = sourceOfName.get(f[2]);
      if (from) { sourceOfName.set(f[1], from); grew = true; }
    }
    if (!grew) break;
  }

  const found = [];
  for (const l of lines) {
    for (const m of l.matchAll(/@Filter\(\s*([A-Za-z_$][\w$]*)[^,]*,\s*"([^"]+)"/g)) {
      const source = sourceOfName.get(m[1]);
      if (!source) continue;
      const fields = fieldsOf(source);
      if (!fields.size || fields.has(m[2])) continue;
      const named = /^\s*([A-Za-z_$][\w$]*)\s*=/.exec(l);
      found.push({ statement: named ? named[1] : null, source, field: m[2] });
    }
  }
  return found;
}

/** Spans of `raw` that sit inside a string literal, so a scan can skip them. */
function quotedSpans(raw) {
  const spans = [];
  let quote = null, start = 0;
  for (let i = 0; i < raw.length; i++) {
    const c = raw[i];
    if (quote) { if (c === quote && raw[i - 1] !== '\\') { spans.push([start, i]); quote = null; } continue; }
    if (c === '"' || c === "'") { quote = c; start = i; }
  }
  return spans;
}

/** Every `AppXxx(...)` call in a line, with its top-level arguments, in order. */
function componentCalls(raw) {
  const skip = quotedSpans(raw);
  const out = [];
  for (const m of raw.matchAll(/\b(App[A-Z][A-Za-z0-9]*)\(/g)) {
    if (skip.some(([a, b]) => m.index > a && m.index < b)) continue;
    const args = topLevelArgs(raw.slice(m.index), m[1]);
    if (args.length) out.push({ name: m[1], args: args.map((a) => a.trim()) });
  }
  return out;
}

const BARE_STATE = /^\$[A-Za-z_][\w$]*$/;
const BARE_IDENT = /^[A-Za-z_][\w$]*$/;

/**
 * A control whose arguments landed in the wrong positional slots.
 *
 * `app-input` has twenty-four positional parameters, `value` at thirteen and
 * `action` at twenty-four, and every generation that has reached for it
 * miscounted. Two independent samples, both of which render a normal-looking
 * box that does nothing at all:
 *
 *   AppInput("md", null, "Agent ID", …, "$agentId", null, runSearch)
 *     -> the state's NAME as a string in `max`, the Action in `pattern`
 *   AppInput("md", null, "Agent ID", …, $agentId, …, lookupAgent)
 *     -> the state in `list`, the Action three past the end and dropped
 *
 * Only the second is caught today, and only incidentally: 27 > 24 trips
 * `excess_arguments` (materialize.js:473), which fires on argument COUNT and
 * never on placement. The first is twenty arguments into a twenty-four
 * parameter signature, so nothing fires — `runSearch` is referenced, so it is
 * not orphaned, and the `@Run` inside it resolves. It was recorded as a clean
 * pass for a surface whose only input is inert.
 *
 * A declared-TYPE check cannot catch either. The catalog types `max`, `step`,
 * `maxlength`, `list`, `spellcheck` and `pattern` all as `string`, so a state
 * or a state's name in any of them is type-valid. What is decidable is the
 * REFERENT: an identifier that names an `Action(...)` statement, and a bare
 * `$state`, are things the catalog says where to put.
 *
 * Structural only. Nothing here asks whether the control should exist, which
 * component it should have been, or what its wording says — those change with
 * taste and these do not.
 */
function positionalContract(statements) {
  const lines = statements.map((st) => st.raw ?? '');

  const actionNames = new Set();
  const stateNames = new Set();
  for (const l of lines) {
    const a = /^\s*([A-Za-z_][\w$]*)\s*=\s*Action\s*\(/.exec(l);
    if (a) actionNames.add(a[1]);
    const s = /^\s*(\$[A-Za-z_][\w$]*)\s*=/.exec(l);
    if (s) stateNames.add(s[1]);
  }

  const found = [];
  for (const l of lines) {
    const named = /^\s*([A-Za-z_$][\w$]*)\s*=/.exec(l);
    const statement = named ? named[1] : null;
    const add = (code, component, param, index, detail) => {
      found.push({ statement, component, code, param, index, detail });
    };

    // A state's NAME in quotes is seven characters of text. Every read of it
    // gets the characters, the box never shows what was typed, and the
    // argument it feeds is asked for an agent literally called "$agentId".
    // Narrow by construction: the name has to be one this surface declares.
    for (const m of l.matchAll(/"(\$[A-Za-z_][\w$]*)"/g)) {
      if (stateNames.has(m[1])) add('state_as_literal', null, null, null, m[1]);
    }

    for (const call of componentCalls(l)) {
      const tag = pascalToTag(call.name);
      const def = catalog.components?.[tag];
      const params = def?.paramOrder ?? [];
      if (!params.length) continue;
      const attrs = def.attributes ?? {};
      const actIdx = params.indexOf('action');
      const bindIdx = params.includes('value') ? params.indexOf('value') : params.indexOf('checked');
      // Only a control that can BE bound is judged on binding. A layout or a
      // display component holding a state is just a component holding a state.
      const interactive = def.actionParam === true && bindIdx >= 0;
      const bound = bindIdx >= 0 && bindIdx < call.args.length
        && /\$[A-Za-z_][\w$]*/.test(call.args[bindIdx]);
      let wiring = false;

      for (let i = 0; i < call.args.length; i++) {
        const a = call.args[i];
        const isAction = BARE_IDENT.test(a) && actionNames.has(a);
        const isState = BARE_STATE.test(a) && stateNames.has(a);
        if (isAction || isState) wiring = true;

        if (isAction && i >= params.length) {
          add('action_dropped', tag, null, i, a);
        } else if (isAction && i !== actIdx) {
          add('action_in_wrong_slot', tag, params[i], i, a);
        }

        // A bare state outside the binding slot, on a control nothing bound.
        // Boolean slots are exempt — `disabled: $busy` is a real thing to
        // write and says nothing about a miscount — and a control whose
        // `value` IS bound is left alone, because a second state elsewhere is
        // then a choice rather than a slip.
        if (isState && interactive && !bound && i !== bindIdx && i < params.length
            && attrs[params[i]]?.type !== 'boolean') {
          add('state_in_non_binding_slot', tag, params[i], i, a);
        }
      }

      // The generalisation of materialize.js's `uncontrolled_input`, which is
      // gated on the ACTION slot being filled — exactly the assumption a
      // miscount breaks, and exactly why the twenty-argument sample was
      // silent. Anything in the list that looks like wiring is enough.
      if (interactive && wiring && !bound) {
        add('control_never_bound', tag, params[bindIdx], bindIdx, null);
      }
    }
  }
  return found;
}

/**
 * The four decisions a server-paged source forces, kept apart.
 *
 * `fetchUsageByAgent(query, page, limit)` returns `{data, total}`, and a
 * request for a paged, searchable list of it has three separate mechanisms to
 * get right and one envelope to keep. Every one of them can be answered the
 * wrong way while the surface renders perfectly:
 *
 *   - page   — a `$state` in the `page` argument, a page SIZE that is the
 *              twenty the request asked for, and an Action in each direction
 *              that moves the state by one and forces the fetch. The
 *              alternative is a frozen `1` with app-table's own pager over
 *              the one page that came back. Both draw a table with page
 *              buttons; only one of them can reach row 21. Checked as
 *              behaviour, never as composition — no component is required,
 *              because which control fires the Action is taste.
 *   - query  — a `$state` in the `query` argument with an Action that @Runs
 *              it, or an @Filter over the fetched page. Both narrow what is
 *              on screen. Only one of them searches the other pages.
 *   - total  — `data.total` is outside `data`, so a Query that selects the
 *              "data" path cannot reach it and the count has to be invented
 *              from the rows in hand.
 *
 * Reported as four independent booleans and never summed. A generation that
 * pages server-side and filters client-side has got one of two mechanisms
 * right, and a single number would say the same thing as one that got
 * neither. The existing structural checks stay separate again: these say
 * which MECHANISM was chosen, not whether the surface is sound.
 *
 * Deliberately NOT part of `expect`. Asserting the answer here would make the
 * question unfalsifiable — it would pass only when the answer was already
 * yes, which is how three prompt experiments in a row measured nothing.
 */
function pagedMechanism(lines, spec) {
  const src = SOURCES_BY_NAME.get(spec.source);
  const argNames = Object.keys(src?.argsShape ?? {});
  const at = (args, name) => {
    const i = argNames.indexOf(name);
    return i >= 0 ? (args[i] ?? '') : '';
  };

  /** Every Query, with its source, its positional argument texts and its path. */
  const queries = [];
  for (const l of lines) {
    const m = /^\s*([A-Za-z_$][\w$]*)\s*=\s*Query\s*\(\s*"([^"]+)"/.exec(l);
    if (!m) continue;
    const parts = topLevelArgs(l, 'Query');
    const list = (parts[1] ?? '').trim().replace(/^\[/, '').replace(/\]$/, '');
    queries.push({
      name: m[1],
      source: m[2],
      args: splitTopLevel(list).map((a) => a.trim()),
      path: (parts[3] ?? '').trim(),
    });
  }
  const mine = queries.filter((q) => q.source === spec.source);
  const detail = {
    sourceUsed: [...new Set(queries.map((q) => q.source))],
    pageArg: null, queryArg: null,
    limitArg: null, limitResolved: null, limitFrom: null,
    next: { action: null, runs: false }, prev: { action: null, runs: false },
    clientPagination: false, clientFilter: false,
  };

  /** `$x = <literal>` — what a state starts as, for resolving a limit. */
  const stateInit = new Map();
  for (const l of lines) {
    const m = /^\s*(\$[\w$]+)\s*=\s*(.+?)\s*$/.exec(l);
    if (m) stateInit.set(m[1], m[2]);
  }
  /** A literal number, or a state that starts as one. Two hops, no cycles. */
  const numberOf = (text, depth = 0) => {
    const t = (text ?? '').trim();
    if (/^-?\d+(?:\.\d+)?$/.test(t)) return Number(t);
    if (depth < 2 && /^\$[\w$]+$/.test(t) && stateInit.has(t)) return numberOf(stateInit.get(t), depth + 1);
    return null;
  };

  // Which Actions set which states, to what, and which Queries they force.
  // The VALUE matters here and not only the name: "next page" and "back to
  // page 1" both @Set the same state and @Run the same Query, and only one
  // of them is a next button.
  const setters = [];
  for (const l of lines) {
    const act = /^\s*([A-Za-z_$][\w$]*)\s*=\s*Action\s*\(/.exec(l);
    if (!act) continue;
    const sets = [];
    for (const m of l.matchAll(/@Set\(/g)) {
      const args = topLevelArgs(l.slice(m.index), '@Set').map((a) => a.trim());
      if (args.length >= 2) sets.push({ state: args[0], value: args[1] });
    }
    setters.push({
      action: act[1],
      sets,
      states: sets.map((x) => x.state),
      runs: [...l.matchAll(/@Run\(\s*([A-Za-z_$][\w$]*)/g)].map((x) => x[1]),
    });
  }
  /** Is `state` written by an Action that also @Runs one of `names`? */
  const drivenBy = (state, names) => setters.some(
    (s) => s.states.includes(state) && s.runs.some((q) => names.includes(q)),
  );

  const names = mine.map((q) => q.name);
  const stateIn = (text) => /^\$[\w$]+$/.test(text.trim()) ? text.trim() : null;

  let searchMechanismCorrect = false;
  for (const q of mine) {
    const page = stateIn(at(q.args, 'page'));
    if (page && !detail.pageArg) {
      detail.pageArg = page;
      // "20 rows at a time" is part of the request, so the page SIZE is part
      // of the mechanism. A frozen 50 pages the data, just not the way it
      // was asked for. An omitted limit is accepted because the service's
      // own default is 20 (usage-service.js:111) — recorded as coming from
      // the default rather than from a decision, so the two stay tellable
      // apart in the corpus.
      const raw = at(q.args, 'limit');
      detail.limitArg = raw === '' ? null : raw;
      if (raw === '' || raw === 'null') { detail.limitResolved = 20; detail.limitFrom = 'service default'; }
      else { detail.limitResolved = numberOf(raw); detail.limitFrom = /^\$/.test(raw.trim()) ? 'state' : 'literal'; }
    }
    const search = stateIn(at(q.args, 'query'));
    if (search) { detail.queryArg = search; if (drivenBy(search, names)) searchMechanismCorrect = true; }
  }

  // "next and previous buttons" is a behaviour, not a composition. What is
  // checked is that some Action moves the page state by one in each
  // direction AND forces the fetch — never which component fires it, which
  // would be a claim about taste rather than about the mechanism. A guarded
  // form (`@Max($page - 1, 1)`) still moves by one and still counts.
  if (detail.pageArg) {
    const pn = detail.pageArg.slice(1);
    const plusOne = new RegExp(`\\$${pn}\\s*\\+\\s*1(?![\\d.])|\\b1\\s*\\+\\s*\\$${pn}\\b`);
    const minusOne = new RegExp(`\\$${pn}\\s*-\\s*1(?![\\d.])`);
    for (const s of setters) {
      const wrote = s.sets.filter((x) => x.state === detail.pageArg).map((x) => x.value).join(' ; ');
      if (!wrote) continue;
      const runs = s.runs.some((q) => names.includes(q));
      // Recorded even when the @Run is missing, so "no next button at all"
      // and "a next button that does not refetch" stay different findings.
      if (plusOne.test(wrote) && !detail.next.action) detail.next = { action: s.action, runs };
      if (minusOne.test(wrote) && !detail.prev.action) detail.prev = { action: s.action, runs };
    }
  }

  const paginationCorrect = Boolean(
    detail.pageArg
    && detail.limitResolved === 20
    && detail.next.action && detail.next.runs
    && detail.prev.action && detail.prev.runs,
  );

  // app-table's own pager over the single page that came back. Not wrong in
  // itself — it is wrong as the ANSWER to "next and previous", which is what
  // makes it worth telling apart from the server mechanism.
  for (const l of lines) {
    for (const call of componentCalls(l)) {
      if (pascalToTag(call.name) !== 'app-table') continue;
      const feeds = names.some((n) => (call.args[0] ?? '').includes(n));
      if (feeds && /"(pages|more)"/.test(call.args[2] ?? '')) detail.clientPagination = true;
    }
  }
  // An @Filter over rows this source already paged narrows the page, not the
  // result set — page two still holds everything the box was meant to hide.
  for (const l of lines) {
    for (const m of l.matchAll(/@Filter\(\s*([A-Za-z_$][\w$]*)/g)) {
      if (names.some((n) => m[1] === n || m[1].startsWith(`${n}.`))) detail.clientFilter = true;
    }
  }
  if (detail.clientFilter) searchMechanismCorrect = false;

  // `total` sits beside `data`, not inside it, so a Query that selects "data"
  // has thrown it away before anything can read it.
  const keepsEnvelope = mine.filter((q) => !q.path || q.path === 'null' || /total/.test(q.path));
  const totalCorrect = keepsEnvelope.some(
    (q) => lines.some((l) => new RegExp(`\\b${q.name}\\.total\\b`).test(l)),
  );

  return {
    sourceCorrect: mine.length > 0,
    paginationCorrect,
    searchMechanismCorrect,
    totalCorrect,
    detail,
  };
}

/** Split a bracket-and-string-aware comma list that is already unwrapped. */
function splitTopLevel(text) {
  const out = [];
  let depth = 0, quote = null, start = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quote) { if (c === quote && text[i - 1] !== '\\') quote = null; continue; }
    if (c === '"' || c === "'") { quote = c; continue; }
    if (c === '(' || c === '[' || c === '{') { depth++; continue; }
    if (c === ')' || c === ']' || c === '}') { depth--; continue; }
    if (c === ',' && depth === 0) { out.push(text.slice(start, i)); start = i + 1; }
  }
  if (text.slice(start).trim()) out.push(text.slice(start));
  return out;
}

/** Every component call with its statement name and its arguments by slot. */
function controlsIn(lines) {
  const out = [];
  for (const l of lines) {
    const named = /^\s*([A-Za-z_$][\w$]*)\s*=/.exec(l);
    for (const call of componentCalls(l)) {
      const tag = pascalToTag(call.name);
      const def = catalog.components?.[tag];
      const params = def?.paramOrder ?? [];
      if (!params.length) continue;
      const slot = {};
      for (let i = 0; i < params.length && i < call.args.length; i++) slot[params[i]] = call.args[i];
      out.push({ statement: named ? named[1] : null, tag, params, slot, args: call.args });
    }
  }
  return out;
}

/**
 * One argument, two ways of asking for it.
 *
 * The corpus says `@Run` production tracks the control kind rather than the
 * source's argument list: across every generated control whose state IS a
 * Query argument, app-search carries `@Run` 31% of the time and every other
 * kind 91%, and inside one case app-search ran 8/18 while
 * app-segmented-control ran 20/20 — sometimes in the same surface. Those are
 * associations in observational data, and control kind was never varied while
 * everything else was held still.
 *
 * This measures exactly that. Two prompts differing only in the control
 * affordance they ask for, over one source, one argument and one required
 * refetch. `fetchSpendCalendar.month` is REQUIRED and is a plain string sent
 * verbatim, so typing "2026-08" and picking "2026-08" are the same request —
 * and a different month's days are simply absent from the response, so there
 * is no client-side mechanism to choose instead. The mechanism decision the
 * earlier experiments tangled with is removed, and what is left is the wiring.
 *
 * Four readings, never summed. A run can name the source, bind the argument
 * and still leave the control inert, and `agent-by-id` is the proof: five
 * runs there authored `@Run` inside an Action that had landed in `pattern`,
 * `step`, `list` or `aria-label`, or past the end of the signature. Counting
 * the token reported 5/10; counting reachable refetches reported 0/10. So
 * `workingRefetchCorrect` asks where the Action actually SITS, not whether
 * the characters `@Run` appear.
 */
function controlPairDimensions(lines, spec) {
  const detail = {
    sourceUsed: [], argState: null, argRaw: null,
    controlUsed: null, controlStatement: null,
    setter: null, actionInSlot: null, setterRunsQuery: false,
    positionalFindings: [], runAuthoredUnreachable: 0, clientFilterOnDate: false,
  };

  const queries = [];
  for (const l of lines) {
    const m = /^\s*([A-Za-z_$][\w$]*)\s*=\s*Query\s*\(\s*"([^"]+)"/.exec(l);
    if (!m) continue;
    const parts = topLevelArgs(l, 'Query');
    queries.push({ name: m[1], source: m[2], args: (parts[1] ?? '').trim() });
  }
  detail.sourceUsed = [...new Set(queries.map((q) => q.source))];
  const mine = queries.filter((q) => q.source === spec.source);
  const sourceCorrect = mine.length > 0;

  // Object call style: the argument is a key. `{month: $month}` binds it;
  // `{month: "2026-09"}` does not, and that difference is the whole point.
  let owner = null;
  for (const q of mine) {
    const m = new RegExp(`\\b${spec.argument}\\s*:\\s*([^,}]+)`).exec(q.args);
    if (!m) continue;
    detail.argRaw = m[1].trim();
    const st = /^(\$[\w$]+)$/.exec(detail.argRaw);
    if (st) { detail.argState = st[1]; owner = q; break; }
  }
  const argumentCorrect = Boolean(detail.argState);

  const controls = controlsIn(lines);
  // The control that BINDS the state, not merely one that mentions it. When
  // the argument was never bound there is still a control to identify —
  // "did it build the kind that was asked for" has to stay answerable
  // independently of "did it reach the argument", or the four readings stop
  // being four.
  const boundTo = (st) => controls.find((c) => {
    const v = c.slot.value ?? c.slot.checked;
    return v && v.trim() === st;
  });
  const bound = (detail.argState && boundTo(detail.argState))
    ?? controls.find((c) => {
      const def = catalog.components?.[c.tag];
      const v = c.slot.value ?? c.slot.checked;
      return def?.actionParam === true && v && /^\$[\w$]+$/.test(v.trim());
    });
  if (bound) { detail.controlUsed = bound.tag; detail.controlStatement = bound.statement; }
  else detail.controlUsed = controls.map((c) => c.tag).join('+') || null;
  const controlKindCorrect = Boolean(bound && bound.tag === spec.control);

  const actions = new Map();
  for (const l of lines) {
    const a = /^\s*([A-Za-z_$][\w$]*)\s*=\s*Action\s*\(/.exec(l);
    if (!a) continue;
    actions.set(a[1], {
      sets: [...l.matchAll(/@Set\(\s*(\$[\w$]+)/g)].map((x) => x[1]),
      runs: [...l.matchAll(/@Run\(\s*([A-Za-z_$][\w$]*)/g)].map((x) => x[1]),
    });
  }
  // An Action nothing can fire. The failure that made an earlier arm read 5/10
  // when the true figure was 0/10.
  const reachable = new Set(controls.map((c) => (c.slot.action ?? '').trim()).filter(Boolean));
  for (const [name, a] of actions) {
    if (a.runs.length && !reachable.has(name)) detail.runAuthoredUnreachable++;
  }

  const findings = positionalContract(lines.map((raw) => ({ raw })));
  detail.positionalFindings = findings
    .filter((f) => bound && f.statement === bound.statement)
    .map((f) => f.code);

  let setter = null;
  if (detail.argState) {
    for (const [name, a] of actions) if (a.sets.includes(detail.argState)) { setter = name; break; }
  }
  detail.setter = setter;
  detail.actionInSlot = bound ? (bound.slot.action ?? null) : null;
  if (setter && owner) detail.setterRunsQuery = (actions.get(setter)?.runs ?? []).includes(owner.name);

  detail.clientFilterOnDate = lines.some((l) => /@Filter\([^,]+,\s*"date"/.test(l));

  const workingRefetchCorrect = Boolean(
    detail.argState                                             // 1
    && setter                                                   // 2
    && detail.actionInSlot && detail.actionInSlot.trim() === setter // 3
    && detail.setterRunsQuery                                   // 4
    && detail.positionalFindings.length === 0,                  // 5
  );

  return { sourceCorrect, argumentCorrect, controlKindCorrect, workingRefetchCorrect, detail };
}

/**
 * INSTRUMENTATION ONLY. Nothing here gates, and nothing here feeds the four
 * readings above.
 *
 * Across 129 recorded AppSearch generations, every one that named its state
 * after a real argument (`$agentId`, `$query`, `$model`) put that state into
 * a Query argument; the ones named after the box (`$modelSearch`,
 * `$agentFilter`) did so 43% of the time. 7/7 against 53/122. That is an
 * association in observational data, and the likelier reading is that the
 * name is a READOUT of a decision already taken rather than an input to it —
 * a generation that has decided to parameterise names the variable after the
 * argument it is about to fill. Recorded because it is the only such signal
 * visible at the point the decision is made, and because batch-to-batch
 * drift in it (one name ran 6% in one batch and 24% across the rest) is
 * otherwise easy to misread as an intervention effect.
 *
 * `...BoundToIntendedQueryArgument` deliberately reuses controlPair's own
 * answer instead of re-deriving one: a second definition of "bound" is a
 * second thing to keep in sync, and this one must never be laxer than the
 * metric it sits beside.
 */
export function appSearchNaming(lines, spec, controlPair) {
  const controls = controlsIn(lines ?? []);
  const boxes = controls.filter((c) => c.tag === 'app-search');
  if (!boxes.length) return null;

  const raw = (boxes[0].slot.value ?? '').trim();
  const state = /^\$[\w$]+$/.test(raw) ? raw : null;

  // Every argument NAME this generation's Query calls put to work: the keys
  // of an options object, plus the declared names behind a positional call,
  // which carries no keys of its own.
  const argNames = new Set();
  const queries = [];
  for (const l of lines ?? []) {
    const m = /^\s*([A-Za-z_$][\w$]*)\s*=\s*Query\s*\(\s*"([^"]+)"/.exec(l);
    if (!m) continue;
    const args = (topLevelArgs(l, 'Query')[1] ?? '').trim();
    queries.push({ name: m[1], source: m[2], args });
    for (const k of args.matchAll(/([A-Za-z_$][\w$]*)\s*:/g)) argNames.add(k[1]);
    for (const n of Object.keys(SOURCES_BY_NAME.get(m[2])?.argsShape ?? {})) argNames.add(n);
  }

  const bare = state ? state.slice(1) : null;
  const matches = Boolean(bare && argNames.has(bare));
  const boundToAny = Boolean(state && queries.some(
    (q) => new RegExp(`\\${state}(?![\\w$])`).test(q.args),
  ));
  const boundToIntended = Boolean(
    state && spec && controlPair?.argumentCorrect
    && controlPair.detail?.argState === state,
  );

  let namingClass = 'other';
  if (matches) namingClass = 'argument_name';
  else if (bare && /Query$/.test(bare)) namingClass = 'query_suffix';
  else if (bare && /Search$/.test(bare)) namingClass = 'search_suffix';

  return {
    appSearchStateName: state,
    appSearchStateMatchesQueryArgument: matches,
    appSearchStateBoundToAnyQueryArgument: boundToAny,
    appSearchStateBoundToIntendedQueryArgument: boundToIntended,
    appSearchStateNamingClass: namingClass,
    ...(boxes.length > 1 ? { appSearchCount: boxes.length } : {}),
  };
}

export function evaluateGeneration(text) {
  const diagnostics = [];
  const { statements, prose } = parseBuffer(text);
  const out = materialize(statements, index, { complete: true });
  diagnostics.push(...out.diagnostics);

  const container = makeEl('div');
  render(out.root, container, catalog, {
    doc: { createElement: makeEl },
    // Every route the app has, so a link is only reported when the model
    // invented a path rather than because this harness has no router.
    routes: { has: (p) => catalog.routes.includes(String(p).split('?')[0]) },
    onDiagnostic: (d) => diagnostics.push(d),
  });

  const tags = [];
  // Which SHAPES of chart, not how many charts. A dashboard answering three
  // different questions with three line plots has drawn one chart three
  // times: the reader learns nothing from the second that the first did not
  // already teach them to read. `type` is unset more often than not, and an
  // unset one is a line — the component's own default (app-chart.js:550).
  const chartKinds = new Set();
  if (container.children[0]) {
    walk(container.children[0], (el) => {
      tags.push(el.tag);
      if (el.tag === 'app-chart') chartKinds.add(el.attrs.type ?? 'line');
    });
  }

  return {
    statements: statements.length,
    lines: statements.map((st) => st.raw ?? ''),
    prose,
    root: out.root,
    tags,
    queries: out.queries,
    mutations: out.mutations,
    states: out.states,
    unresolved: out.unresolved,
    actions: statements.filter((s) => /=\s*Action\(/.test(s.raw ?? '')).length,
    redundantRuns: redundantRuns(statements),
    missingQueryRuns: missingQueryRuns(statements),
    unknownFilterFields: unknownFilterFields(statements),
    positionalContract: positionalContract(statements),
    semanticMismatches: semanticMismatches(statements),
    chartKinds: [...chartKinds],
    diagnostics,
  };
}

/**
 * Two verdicts, deliberately, because the offline gate and a live run ask
 * different questions of the same text.
 *
 * `--offline` replays a RECORDING. Its job, in this file's own words at the
 * top, is that "a change to the runtime or the catalog is checked against real
 * model output on every commit" — a regression test over a frozen sample. But
 * the checker it runs also asserts whether the generation was any GOOD, and
 * those two went into one list. So a weak draw failed CI forever: not because
 * anything regressed, but because that recording was poor, and the only ways
 * out were to re-record until the sample flattered us, or to annotate the case
 * as a known failure. Both hide the thing the gate exists to see.
 *
 * `fail` is the contract: this DSL no longer becomes a working surface under
 * today's code. A root that does not resolve, a fatal render or materialize
 * diagnostic, a reference to a statement that is not there, a source the scope
 * has withdrawn. Each of those can newly break with the fixture untouched, and
 * each means OUR side moved. That is what should stop a build.
 *
 * `quality` is what the recording says about the generator: the case's own
 * minimums, the wiring readings, the positional contract, the filter fields.
 * Real findings, printed and counted — but frozen with the fixture. They
 * cannot newly fail, so gating on them preserves whichever draw happened to
 * be committed, which is a verdict about history rather than about this
 * commit.
 *
 * A live or `--record` run counts both, because there quality IS the
 * measurement and a fresh draw can genuinely be worse than the last.
 *
 * @returns {{fail: string[], quality: string[], advisory: string[], runtime: string[]}}
 */
export function check(kase, text) {
  const r = evaluateGeneration(text);
  /** Contract — this DSL is broken under today's runtime and catalog. */
  const fail = [];
  /** Generation quality — always reported, gates only a live run. */
  const quality = [];
  /** Corrected, not broken — shown, never fatal. */
  const advisory = [];
  const runtime = [];
  const e = kase.expect ?? {};
  // Which mechanism was chosen, reported apart from whether the surface is
  // sound. Never added to `fail`: a case that gates on the answer it is
  // measuring can only ever return the answer it was given.
  const dimensions = e.mechanism ? pagedMechanism(r.lines, e.mechanism) : null;
  // The control-kind pair. Same four-readings-never-summed discipline as
  // `dimensions`, and kept in its own field so neither case's numbers can be
  // mistaken for the other's.
  const controlPair = e.controlPair ? controlPairDimensions(r.lines, e.controlPair) : null;
  // Instrumentation, computed for every case that built an AppSearch —
  // including the ones with no mechanism expectation, where only the
  // intended-argument reading is unavailable.
  const searchNaming = appSearchNaming(r.lines, e.controlPair ?? null, controlPair);

  if (e.noSurface) {
    if (r.root) quality.push('built a dashboard for a question that should have been answered in prose (rule 11)');
    if (!r.prose.join('').trim()) quality.push('answered with nothing at all');
    return { fail, quality, advisory, runtime, dimensions, controlPair, searchNaming, r };
  }

  if (!r.root) {
    if (e.allowNoSurface) return { fail, quality, advisory, runtime, dimensions, controlPair, searchNaming, r };
    fail.push('no root — nothing rendered');
  }

  // These are never acceptable, whatever the case asked for.
  for (const d of r.diagnostics) {
    const line = `diagnostic ${d.source}/${d.code}: ${d.message}`;
    const severity = severityOf(d.code);
    if (severity === 'fatal') fail.push(line);
    else if (severity === 'runtime') runtime.push(line);
    else advisory.push(line);
  }
  for (const name of r.unresolved) fail.push(`references "${name}", which is not defined`);
  for (const q of r.queries) {
    if (!ALLOWED_SOURCES.has(q.source)) fail.push(`Query names "${q.source}", which the scope does not allow`);
    // Quality, not contract, and the guard below is what said so: this was
    // written against `fail` before the two lists existed, the merge that
    // brought them together had no textual conflict, and the contract count
    // failed at five. The reading is severe — the source rejects the call and
    // the panel shows a failure — but it is severe about THIS generation, and
    // a replay cannot newly discover it. A live run still gates on it.
    for (const bad of argumentsOutsideEnum(q)) {
      quality.push(`Query passes ${bad.argument}="${bad.value}" to ${bad.source}, which takes one of `
        + `${bad.allowed.join(', ')} — the call is refused and the component renders a failure`);
    }
  }
  if (r.mutations.length) quality.push(`wrote ${r.mutations.length} Mutation(s); no write-capable source exists`);

  if (e.minQueries && r.queries.length < e.minQueries) {
    quality.push(`${r.queries.length} queries, expected at least ${e.minQueries} — a dashboard with no real data`);
  }
  if (e.minComponents && r.tags.length < e.minComponents) {
    quality.push(`${r.tags.length} components, expected at least ${e.minComponents}`);
  }
  if (e.minActions && r.actions < e.minActions) {
    quality.push(`${r.actions} Actions, expected at least ${e.minActions}`);
  }
  if (e.minStates && r.states.length < e.minStates) {
    quality.push(`${r.states.length} $state variables, expected at least ${e.minStates}`);
  }
  for (const tag of e.tags ?? []) {
    if (!r.tags.includes(tag)) quality.push(`no <${tag}> anywhere in the tree`);
  }
  // Advisory, not a failure. The defect is real and the detection is narrow,
  // but it is a claim about what the DATA will do rather than about what
  // rendered, so it is reported and measured before it is allowed to gate.
  for (const sm of r.semanticMismatches) {
    advisory.push(`diagnostic eval/semantic_control_argument_mismatch: ${sm.control} writes `
      + `${sm.state}, passed to ${sm.source}'s "${sm.argument}" (${sm.query}). ${sm.reason}`);
  }
  for (const uf of r.unknownFilterFields) {
    quality.push(`${uf.statement ?? '@Filter'} filters on "${uf.field}", which ${uf.source} does not `
      + 'return — it matches nothing, every time, and the surface renders as if there were no data');
  }
  // Quality, not contract. The surface renders; the control in it cannot do
  // the one thing it is there for — a statement about this generation, not
  // about whether today's runtime still handles this DSL. See check()'s
  // docblock for why the two are counted apart.
  for (const pc of r.positionalContract) {
    const where = pc.statement ? `${pc.statement}: ` : '';
    const at = pc.param ? `"${pc.param}" (slot ${pc.index + 1})` : `slot ${pc.index + 1}`;
    if (pc.code === 'action_in_wrong_slot') {
      quality.push(`${where}${pc.component} takes its Action last, but ${pc.detail} is at ${at} — `
        + 'the control has no action and that slot holds something it cannot use');
    } else if (pc.code === 'action_dropped') {
      quality.push(`${where}${pc.component} has ${pc.detail} at ${at}, past the end of its parameter `
        + 'list, so the Action is dropped and the control does nothing');
    } else if (pc.code === 'state_in_non_binding_slot') {
      quality.push(`${where}${pc.component} has ${pc.detail} at ${at}, which is not where a value is `
        + 'bound — the arguments are off by a slot and what the user types goes nowhere');
    } else if (pc.code === 'control_never_bound') {
      quality.push(`${where}${pc.component} is wired to a state or an Action but its ${at} is not `
        + 'read back from a $state, so what the user types is discarded on the next repaint');
    } else if (pc.code === 'state_as_literal') {
      quality.push(`${where}"${pc.detail}" is the state's NAME in quotes, not the state — every read `
        + `of it gets those characters, and ${pc.detail} itself is never read`);
    }
  }
  for (const mr of r.missingQueryRuns) {
    quality.push(`${mr.action} sets ${mr.state}, which ${mr.query} reads as an argument, but does `
      + 'not @Run it — a $state changing never re-fetches on its own, so the control moves and '
      + 'the data does not. Either @Run it, or take the state out of the arguments and filter '
      + 'what is already fetched');
  }
  for (const rr of r.redundantRuns) {
    quality.push(`${rr.action} re-runs ${rr.query}, but ${rr.states.join('/')} is not one of its `
      + 'arguments — the fetch returns identical data and nothing on screen changes. '
      + 'Either put the state in the Query\'s arguments or drop the @Run and filter what is '
      + 'already fetched');
  }
  if (e.minChartKinds && r.chartKinds.length < e.minChartKinds) {
    quality.push(`${r.chartKinds.length} kind(s) of chart (${r.chartKinds.join(', ') || 'none'}), `
      + `expected at least ${e.minChartKinds} — the same shape repeated answers one question twice`);
  }

  return { fail, quality, advisory, runtime, dimensions, controlPair, searchNaming, r };
}

/**
 * Where generation is reached, and as whom.
 *
 * Weave is no longer a process this script talks to. It is a deployed agent —
 * a normal `agents` row with `is_internal = true`, seeded by the control plane
 * from WEAVE_AGENT_IMAGE — and the EE Weave surface is the only
 * way in: it resolves that agent, speaks A2A `message/stream` to it, and
 * translates the frames back into the same SSE contract this script already
 * reads. So the wire format below is unchanged; only the host and the auth
 * moved.
 *
 * Which means the old shared secret is gone. `/api/weave/surface` is behind
 * `require_auth` like any other control-plane route, so this logs in as a real
 * user. A platform capability every logged-in user gets, not a resource with
 * its own key.
 */
const CP_BASE = process.env.NASIKO_CP_BASE_URL || 'http://localhost:8082';

let cachedToken = null;

/** A bearer token for the control plane, fetched once per run. */
async function login() {
  if (cachedToken) return cachedToken;
  const username = process.env.NASIKO_ADMIN_USERNAME;
  const password = process.env.NASIKO_ADMIN_PASSWORD;
  if (!username || !password) {
    throw new Error('NASIKO_ADMIN_USERNAME / NASIKO_ADMIN_PASSWORD are not set in this shell');
  }
  const res = await fetch(`${CP_BASE}/api/auth/login`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ username, password }),
  });
  if (!res.ok) {
    throw new Error(`login to ${CP_BASE} returned ${res.status} — are the admin credentials right?`);
  }
  cachedToken = (await res.json()).token;
  if (!cachedToken) throw new Error(`login to ${CP_BASE} returned no token`);
  return cachedToken;
}

/**
 * One id per case per run, which is what the route calls the A2A context.
 *
 * Per CASE because a session is a conversation: two cases sharing one would
 * let the dashboard generated for the first steer the second, and the suite
 * would stop measuring twelve independent prompts. Per RUN because the agent
 * keeps that context — a stable id would have every re-record answering with
 * a month of accumulated history behind it.
 *
 * The repair turn is the deliberate exception: it passes the id of the turn
 * it is repairing, because patching by statement name only means anything
 * against the surface that produced those names.
 */
const RUN = Date.now().toString(36);
const sessionFor = (id) => `eval-${id}-${RUN}`;

/** Read one generation off the control plane, concatenating its dsl-chunks. */
async function generate(prompt, { currentSurface, sessionId, model } = {}) {
  const base = CP_BASE;
  const token = await login();
  const res = await fetch(`${base}/api/weave/surface`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      accept: 'text/event-stream',
      authorization: `Bearer ${token}`,
    },
    body: JSON.stringify({
      // Required, and the request struct is deny_unknown_fields, so this is
      // both mandatory and unforgiving: a missing one is a 400 before the
      // prompt is looked at. Added to the route on 2026-09-13 (dec454e5) for
      // persisted chat history; this script never sent one and kept passing
      // against control planes built before that, which is exactly as long as
      // it took someone to rebuild.
      session_id: sessionId ?? sessionFor('adhoc'),
      prompt,
      context: {
        catalogVersion: catalog.catalogVersion,
        // A repair turn patches by statement name, which only works if the
        // generator can see what it is patching. The browser sends this on
        // every turn (surface-stream.js); without it here the model would
        // have to reproduce the whole dashboard to change one line.
        ...(currentSurface && { currentSurface }),
        // A key from generation-models.json, checked by the route. Absent
        // means the route's default — which is what every run before
        // --model existed recorded, so the gate's fixtures stay comparable.
        ...(model && { model }),
      },
    }),
  });
  // 503 is the route saying the agent is not reachable — worth separating from
  // a transport failure, because the fix is a deployment, not a retry.
  //
  // The route knows WHICH of the two ways it is unreachable (never seeded, or
  // seeded and the deploy failed) and says so in the body, so print that
  // rather than the guess this used to make. It told twelve cases in a row to
  // go set WEAVE_AGENT_IMAGE on a deployment where WEAVE_AGENT_IMAGE was set
  // and the image was in the registry — advice that is not merely useless but
  // points away from the startup log, which is where the cause actually is.
  if (res.status === 503) {
    const said = await res.text().then(
      (t) => { try { return JSON.parse(t).error ?? ''; } catch { return ''; } }, () => '');
    const err = new Error(
      `${said || 'the control plane has no running weave agent'} `
      + '— check the control plane\'s startup output for "weave agent"');
    // Flagged rather than only printed, because the caller can act on it. A
    // 503 here is always "no agent to talk to", and the one case where that
    // resolves by itself is a control plane that has only just started: the
    // agent row sits at 'deploying' while the image is pulled and the
    // container comes up, which can outlast the seconds between `just run`
    // and the next command. Twelve cases were spent on exactly that.
    err.agentUnavailable = true;
    throw err;
  }
  if (res.status === 404) {
    throw new Error(
      `${base} has no /api/weave/surface route — that route is EE-only, so this `
      + 'has to be the EE server (`just run`), not the OSS one');
  }
  if (!res.ok) {
    // The route answers a JSON {error} on every 4xx and this used to drop it,
    // so twelve identical "HTTP 400" lines said nothing about which of the six
    // rejections fired. The body names it in one word.
    const said = await res.text().then(
      (t) => { try { return JSON.parse(t).error ?? t.slice(0, 200); } catch { return t.slice(0, 200); } },
      () => '');
    throw new Error(
      `HTTP ${res.status} from ${base}/api/weave/surface${said ? ` — ${said}` : ''}`);
  }

  let text = '';
  let generatorCatalog = null;
  /** The generator's own account of itself — see PROVENANCE_KEYS. */
  let generator = null;
  // The stream says why it produced nothing, and this used to drop it on the
  // floor: everything that was not a dsl-chunk was skipped, so an agent that
  // failed outright reported as "answered with nothing at all" and the actual
  // message — an executor failure, a refused model call — never left the wire.
  const failures = [];
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buf = '';
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    buf += decoder.decode(value, { stream: true });
    const frames = buf.split('\n\n');
    buf = frames.pop() ?? '';
    for (const frame of frames) {
      const event = frame.match(/^event:\s*(.+)$/m)?.[1]?.trim();
      const data = frame.match(/^data:\s*(.+)$/m)?.[1];
      if (!data) continue;
      if (event === 'surface') {
        try {
          const meta = JSON.parse(data);
          generatorCatalog = meta.catalogVersion ?? null;
          generator = provenanceFrom(meta);
        } catch { /* reported below */ }
        continue;
      }
      if (event === 'fail') {
        try {
          const f = JSON.parse(data);
          failures.push(f.message ? `${f.code ?? 'fail'}: ${f.message}` : JSON.stringify(f));
        } catch { failures.push(data); }
        continue;
      }
      if (event !== 'dsl-chunk') continue;
      try { text += JSON.parse(data).text ?? ''; } catch { /* a truncated frame is the client's problem too */ }
    }
  }
  // A failure frame with no DSL behind it is the whole answer, so it is raised
  // rather than returned — the per-case "no root, 0 queries" lines underneath
  // describe the symptom and this is the cause.
  if (!text.trim() && failures.length) {
    throw new Error(`the generator failed — ${failures.join('; ')}`);
  }
  return { text, generatorCatalog, generator, failures };
}

// ── main ────────────────────────────────────────────────────────────────────
// Guarded so the checker above can be imported and tested without a model, a
// network, or a process exit.

const invokedDirectly = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (!invokedDirectly) { /* imported for its checker */ } else {

const args = process.argv.slice(2);
const offline = args.includes('--offline');
const record = args.includes('--record');
/**
 * Also measure the repair turn (common/surface/repair.js).
 *
 * Off by default because it doubles the requests on a run that is already
 * slow, and because the fixtures this records are FIRST-PASS output — that is
 * what the offline gate should keep judging. The repair number is reported
 * separately and answers a different question: not "is the model good", but
 * "when it is wrong, can it fix itself from what the renderer told it".
 *
 * Advisories are included here and excluded in the browser, deliberately. A
 * live user should not wait a round trip to tidy a pre-fetch placeholder;
 * an eval measuring whether the loop works should absolutely count them,
 * since default_is_whole_response is the single most common thing it has to
 * repair.
 */
const withRepair = args.includes('--repair');
const only = args[args.indexOf('--case') + 1];
const cases = only && args.includes('--case') ? CASES.filter((c) => c.id === only) : CASES;
/** Diff the current fixtures against a saved run. Analysis, never a gate. */
const compareTo = args.includes('--compare') ? args[args.indexOf('--compare') + 1] : null;
/**
 * `--model <key>` — record under one of the models in generation-models.json.
 *
 * The fixtures under tests/fixtures/generations/ are what the offline gate
 * judges on every push, and they are the production model's output. A run
 * under any OTHER model therefore writes its run directory only and never
 * overwrites a fixture: otherwise one comparison run would silently change
 * which model CI is judging, and the next replay would report the switch as a
 * regression or an improvement that no code change caused.
 *
 * Validated here against the same file the route is held to, so a typo is a
 * usage error before a single model call rather than a 400 per case.
 */
const MODELS = JSON.parse(readFileSync(resolve(UI, 'common/surface/generation-models.json'), 'utf8'));
const modelKey = args.includes('--model') ? args[args.indexOf('--model') + 1] : null;
if (modelKey != null && !MODELS.models.some((m) => m.key === modelKey)) {
  console.error(`eval: --model "${modelKey}" is not in generation-models.json `
    + `(known: ${MODELS.models.map((m) => m.key).join(', ')})`);
  process.exit(2);
}
if (modelKey && offline) {
  console.error('eval: --model only applies to --record; --offline replays the recorded fixtures as they are.');
  process.exit(2);
}
/** True when this run must not touch the gate's fixtures. */
const comparisonOnly = Boolean(modelKey && modelKey !== MODELS.default);

if (!existsSync(FIXTURES)) mkdirSync(FIXTURES, { recursive: true });

/**
 * `--compare <dir>` — what changed between a saved run and the fixtures now.
 *
 * Reads both sides off disk and re-evaluates them, so it works against any
 * directory of `.dsl` files: a run under `tests/fixtures/runs/`, a baseline
 * copied out of git, anything. No network, no model, deterministic.
 *
 * Deliberately NOT an assertion and deliberately not wired into CI. Breadth
 * moves by a component or two between two runs of an unchanged prompt — a
 * gate on that number would fail on noise. This exists to answer "did the
 * intervention convert the cases it was aimed at", which is legible per case
 * and is the signal the corpus number is too coarse to carry.
 */
if (compareTo) {
  if (!compareTo || !existsSync(compareTo)) {
    console.error(`eval --compare: no such directory: ${compareTo ?? '(missing argument)'}`);
    process.exit(2);
  }
  const before = evaluateDir(compareTo);
  const after = evaluateDir(FIXTURES);
  const baselineProvenance = readProvenance(compareTo);
  const ids = [...new Set([...Object.keys(before), ...Object.keys(after)])].sort();

  const bKinds = new Set(Object.values(before).flatMap((c) => c.kinds));
  const aKinds = new Set(Object.values(after).flatMap((c) => c.kinds));
  const gained = [...aKinds].filter((k) => !bKinds.has(k)).sort();
  const lost = [...bKinds].filter((k) => !aKinds.has(k)).sort();

  console.log(`eval --compare\n  baseline: ${compareTo}\n  current : ${FIXTURES}\n`);
  // Without this the reader has to remember what was deployed when, which is
  // exactly the thing that made the last A/B unattributable.
  if (baselineProvenance) {
    console.log('baseline generator'
      + `\n  prompt ${baselineProvenance.promptDigest ?? '?'}`
      + `   spec ${baselineProvenance.specDigest ?? '?'}`
      + `   code ${baselineProvenance.generatorDigest ?? '?'}`
      + `   model ${baselineProvenance.model ?? '?'}`);
    if (baselineProvenance.consistent === false) {
      console.log('  NOT one condition — that run was recorded across a redeploy');
    }
    console.log('  (record a new run to print the current generator beside it)\n');
  } else {
    console.log('baseline generator: not recorded — that run predates provenance, '
      + 'so any difference below cannot be attributed to a prompt change\n');
  }
  console.log(`corpus breadth  ${bKinds.size}/${CATALOG_SIZE}  ->  ${aKinds.size}/${CATALOG_SIZE}`);
  console.log(`  newly generated : ${gained.join(', ') || 'none'}`);
  console.log(`  no longer used  : ${lost.join(', ') || 'none'}\n`);

  const unchanged = [];
  for (const id of ids) {
    const b = before[id]; const a = after[id];
    if (!b) { console.log(`+ ${id}  (new case) ${a.kinds.join(' ')}`); continue; }
    if (!a) { console.log(`- ${id}  (gone from the current fixtures)`); continue; }
    const plus = a.kinds.filter((k) => !b.kinds.includes(k));
    const minus = b.kinds.filter((k) => !a.kinds.includes(k));
    const dPlus = Object.keys(a.diagnostics).filter((c) => !b.diagnostics[c]);
    const dMinus = Object.keys(b.diagnostics).filter((c) => !a.diagnostics[c]);
    if (!plus.length && !minus.length && !dPlus.length && !dMinus.length
        && b.instances === a.instances) { unchanged.push(id); continue; }
    console.log(`~ ${id}`);
    console.log(`    kinds     ${b.kinds.length} -> ${a.kinds.length}`
      + `${plus.length ? `   +${plus.join(' +')}` : ''}${minus.length ? `   -${minus.join(' -')}` : ''}`);
    if (b.instances !== a.instances) console.log(`    instances ${b.instances} -> ${a.instances}`);
    if (b.fatal !== a.fatal) console.log(`    fatal     ${b.fatal} -> ${a.fatal}`);
    if (dPlus.length) console.log(`    new diagnostics      ${dPlus.join(', ')}`);
    if (dMinus.length) console.log(`    resolved diagnostics ${dMinus.join(', ')}`);
  }
  if (unchanged.length) console.log(`\n= unchanged (${unchanged.length}): ${unchanged.join(', ')}`);
  console.log('\nObservational. Generation is stochastic — a difference here is a '
    + 'hypothesis, not a result, until it repeats.');
  process.exit(0);
}

if (offline) {
  const have = new Set(readdirSync(FIXTURES).filter((f) => f.endsWith('.dsl')).map((f) => f.replace(/\.dsl$/, '')));
  const missing = cases.filter((c) => !have.has(c.id));
  if (missing.length && have.size === 0) {
    // Nothing recorded at all — there is no baseline yet, so there is nothing
    // to regress against and failing would just park CI on red for a reason
    // nobody can fix without a live model. Say so once and pass.
    console.log('eval: no recordings yet — run with --record against a live Weave to create a baseline.');
    process.exit(0);
  }
  if (missing.length) {
    // Some recordings exist and these do not, which means a case was added
    // without one. That is a gap somebody introduced, so it fails.
    console.error(`eval: no recording for ${missing.map((c) => c.id).join(', ')}, but others exist.`);
    console.error('A case without a recording is a case that never runs. Record it, or remove it.');
    process.exit(1);
  }
}

// Credentials for the control plane, checked once rather than per case.
if (!offline && !(process.env.NASIKO_ADMIN_USERNAME && process.env.NASIKO_ADMIN_PASSWORD)) {
  console.error('eval: NASIKO_ADMIN_USERNAME / NASIKO_ADMIN_PASSWORD are not set in this shell.\n');
  console.error('  set -a && source ~/Documents/GitHub/Weave/.env && set +a\n');
  console.error('Generation goes through the control plane now, not straight at Weave:');
  console.error(`  ${process.env.NASIKO_CP_BASE_URL || 'http://localhost:8082'}/api/weave/surface`);
  console.error('so it needs a login, not the old WEAVE_INTERNAL_TOKEN. Weave itself is a');
  console.error('deployed agent the control plane seeds from WEAVE_AGENT_IMAGE — there is');
  console.error('nothing to start by hand.');
  process.exit(1);
}

/**
 * The agent has to be looking at THIS repo's catalog, not its bundled fallback.
 *
 * Checked before spending eleven model calls, because the failure is quiet at
 * both ends. Weave's catalog.py falls back to the copy inside the image when
 * WEAVE_CATALOG_URL will not resolve, and the agent runs in a container where
 * `localhost` is the container — so the control plane has to hand it a URL
 * that resolves from in there, and nothing says so when it doesn't.
 *
 * The per-case guard below compares catalog versions and catches this when
 * the bundled copy is stale. It cannot catch it when the bundle happens to be
 * current, which is exactly the state the bundle is in right after someone
 * syncs it — so the versions match, the run looks clean, and the generator
 * never once talked to this control plane. This checks reachability instead
 * of equality, which does not have that hole.
 */
async function checkCatalogReachable() {
  // Not this shell's variable any more — the control plane passes it to the
  // agent container it seeds (oss/server/src/seed.rs). Checked here anyway,
  // because what has to be true is the same: something is serving this
  // catalog, at a URL the agent can resolve.
  const url = process.env.WEAVE_CATALOG_URL
    || `${CP_BASE}/common/surface/dsl-catalog.json`;
  // Fetched from here, not from inside Weave — so this proves the URL serves a
  // catalog, not that Weave can reach it. A host.docker.internal URL is not
  // resolvable from this process at all, which is the common and correct case;
  // it is reported rather than failed.
  let hostUrl = url;
  let viaDockerHost = false;
  if (url.includes('host.docker.internal')) {
    hostUrl = url.replace('host.docker.internal', 'localhost');
    viaDockerHost = true;
  }
  try {
    const res = await fetch(hostUrl);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const served = (await res.json()).catalogVersion;
    if (served !== catalog.catalogVersion) {
      console.error(`eval: ${hostUrl} serves catalog ${served}, this checkout has ${catalog.catalogVersion}.`);
      console.error('The control plane is running an older build of this repo — restart it.');
      process.exit(1);
    }
    console.log(`eval: catalog ${served} served at ${hostUrl}${viaDockerHost ? ' (the agent reaches it as host.docker.internal)' : ''}`);
  } catch (err) {
    console.error(`eval: the catalog at ${url} did not answer — ${err.message}.\n`);
    console.error('Start the control plane with `just run`. It binds CP_BIND from');
    console.error('the server env file, and both this check and the seeded agent read from it,');
    console.error('so the port there is the one that has to be right.');
    if (viaDockerHost) {
      console.error(`\n(Tried ${hostUrl} from here — host.docker.internal only resolves`);
      console.error('inside the container, which is where it has to work.)');
    }
    process.exit(1);
  }
}

if (!offline) await checkCatalogReachable();

/**
 * Retry through a control plane whose weave agent has not finished deploying.
 *
 * The seeded agent's row is set to 'deploying' before its image is pulled and
 * only becomes 'running' once the container answers, so a run started in the
 * same breath as `just run` gets a 503 on every case and records nothing —
 * twelve model calls' worth of nothing, and a run directory with breadth 0/41
 * that looks like a catastrophic regression rather than a race.
 *
 * Bounded, and spent ONCE for the whole run rather than per case: if the agent
 * is genuinely failed, waiting longer will not fix it, and the budget is there
 * to tell "not yet" apart from "not going to". The wait is announced, because
 * a script that silently stalls for two minutes is worse than one that fails.
 */
const AGENT_WAIT_MS = 150_000;
let agentWaitLeft = AGENT_WAIT_MS;
let announcedWait = false;
async function withAgentReady(attempt) {
  for (;;) {
    try {
      return await attempt();
    } catch (err) {
      if (!err.agentUnavailable || agentWaitLeft <= 0) throw err;
      if (!announcedWait) {
        announcedWait = true;
        console.log(`eval: the weave agent is not answering yet — ${err.message}`);
        console.log(`  waiting up to ${Math.round(AGENT_WAIT_MS / 1000)}s for it to come up, `
          + 'rather than spending the corpus on a control plane that has only just started.');
      }
      await new Promise((r) => setTimeout(r, 5_000));
      agentWaitLeft -= 5_000;
      if (agentWaitLeft <= 0) {
        console.log('  still not answering. Treating it as deployed-and-failed from here on.');
      }
    }
  }
}



/**
 * This run's own directory, and the per-case rows that become `summary.json`.
 *
 * The run's generations are written HERE as well as over the fixtures, so the
 * directory is a faithful record of this run rather than of the one it
 * replaced. A run is then identifiable by its own content: each row carries a
 * sha256 of the exact text that was evaluated.
 */
const runDir = record
  ? resolve(RUNS, `${new Date().toISOString().replace(/[:.]/g, '-').replace(/-\d+Z$/, 'Z')}-${catalog.catalogVersion}`
    + (comparisonOnly ? `-${modelKey}` : ''))
  : null;
if (comparisonOnly) {
  console.log(`eval: recording under --model ${modelKey} — a comparison run. It is saved to its own run `
    + `directory and the gate's fixtures are left as they are.`);
}
if (runDir) mkdirSync(runDir, { recursive: true });
/** One row per case, in case order. */
const runRows = {};

/** Cases whose recording was refused because the generator answered nothing. */
const skipped = [];
let failed = 0;
/** case id -> the generator's own provenance for that generation. */
const caseProvenance = {};
/** What the repair turn did, when --repair asked for one. */
const repairs = { offered: 0, cleared: 0, improved: 0, noBetter: 0, before: 0, after: 0 };
/** Cases whose request never came back — a transport or route failure, not a sample. */
const unreachable = [];
for (const kase of cases) {
  const path = resolve(FIXTURES, `${kase.id}.dsl`);
  let text;
  try {
    if (offline) {
      text = readFileSync(path, 'utf8');
    } else {
      const got = await withAgentReady(() => generate(kase.prompt, { sessionId: sessionFor(kase.id), model: modelKey }));
      text = got.text;
      caseProvenance[kase.id] = got.generator;
      // The generator says which catalog it built against. Judging its output
      // with a different one is judging the wrong thing — and a silent
      // fallback to a stale bundled copy looks exactly like a model that will
      // not follow a rule, which cost three rounds of prompt edits before
      // anyone thought to check.
      if (got.generatorCatalog && got.generatorCatalog !== catalog.catalogVersion) {
        console.error(`\neval: the generator built against catalog ${got.generatorCatalog}, this repo has ${catalog.catalogVersion}.`);
        console.error('Every result below would be judged against a vocabulary the generator never saw.');
        console.error('Point WEAVE_CATALOG_URL at this control plane and restart Weave.');
        process.exit(1);
      }
    }
  } catch (err) {
    console.error(`✗ ${kase.id}: ${err.message}`);
    failed++;
    // Counted separately from `skipped`, which means "answered, but with
    // nothing". This one never reached the generator at all, and the two used
    // to be conflated by omission: a case that threw fell straight past the
    // record block, so `skipped` stayed empty, the "your fixtures are
    // untouched" reassurance never fired, and the summary went on to report
    // `cases.length - skipped.length` files recorded. Twelve 400s printed
    // "recorded 12" while writing nothing — the one moment the count had to
    // be right.
    unreachable.push(kase.id);
    if (record) runRows[kase.id] = { status: 'unreachable', error: err.message };
    continue;
  }
  // An empty answer is never a recording worth keeping. It means the generator
  // could not answer — no model credential, no skill registered, a refused
  // connection — and none of those are a sample of anything. Writing it
  // destroys a good fixture and replaces it with a file that then fails the
  // offline run for a reason that has nothing to do with the generation.
  //
  // Learned the hard way: a blank AWS_BEARER_TOKEN_BEDROCK in .env.docker
  // emptied all eleven in one command, including the prose-only cases that
  // touch neither the catalog nor the DSL.
  if (record) {
    if (text.trim()) {
      if (!comparisonOnly) writeFileSync(path, text);
      // The same bytes into the run directory. Written here rather than by
      // copying the fixtures afterwards, so a case that is skipped below
      // (empty answer, fixture left alone) is absent from the snapshot too
      // instead of silently carrying the previous run's text.
      writeFileSync(resolve(runDir, `${kase.id}.dsl`), text);
    } else skipped.push(kase.id);
  }

  const { fail, quality, advisory, runtime, dimensions, controlPair, searchNaming, r } = check(kase, text);
  // A live run is sampling the generator, so a weak draw is the result and
  // counts. A replay is asking whether today's code still handles a draw that
  // was already judged when it was recorded, so only the contract counts —
  // see check()'s docblock.
  const gating = offline ? fail : [...fail, ...quality];

  // Observational only — nothing below reads this to decide pass or fail.
  if (record) {
    const { counts, fatal } = diagnosticTally(r);
    runRows[kase.id] = {
      status: gating.length ? 'failed' : (text.trim() ? 'ok' : 'skipped'),
      failReasons: fail,
      qualityFindings: quality,
      sha256: sha256(text),
      statements: r.statements,
      componentInstances: (r.tags ?? []).length,
      uniqueKinds: kindsOf(r),
      uniqueKindCount: kindsOf(r).length,
      chartKinds: r.chartKinds ?? [],
      queries: (r.queries ?? []).length,
      actions: r.actions ?? 0,
      states: (r.states ?? []).length,
      diagnostics: counts,
      fatalDiagnostics: fatal,
      generator: caseProvenance[kase.id] ?? null,
      // Four booleans, never summed. Absent for a case with no mechanism
      // expectation, rather than four falses that would read as four
      // failures.
      ...(dimensions ? { dimensions } : {}),
      ...(controlPair ? { controlPair } : {}),
      // Naming instrumentation. Absent when the generation built no
      // AppSearch, so a missing key never reads as a false.
      ...(searchNaming ? { searchNaming } : {}),
      // The structural checks, kept beside them and kept apart from each
      // other: which mechanism was chosen and whether the surface is sound
      // are different questions, and a benchmark that merges them cannot say
      // which half moved.
      structural: {
        missingQueryRuns: (r.missingQueryRuns ?? []).length,
        redundantRuns: (r.redundantRuns ?? []).length,
        unknownFilterFields: (r.unknownFilterFields ?? []).length,
        positionalContract: (r.positionalContract ?? []).map((x) => x.code),
        orphanedStatements: (r.diagnostics ?? []).filter((d) => d.code === 'orphaned_statement').length,
        unresolved: (r.unresolved ?? []).length,
        fatalDiagnostics: fatal,
        // An Action holding an @Run that nothing can fire. Counting the token
        // instead of the wire reported 5/10 on a case whose true figure was
        // 0/10, so it is recorded on every run, not only the paired ones.
        runAuthoredUnreachable: controlPair?.detail.runAuthoredUnreachable ?? null,
      },
    };
  }

  // ── The repair turn, measured ───────────────────────────────────────────
  // Hand the renderer's own diagnostics back and see whether the generator
  // can fix them. Deliberately AFTER the fixture is written, so what the
  // offline gate keeps judging is first-pass output — this measures recovery,
  // which is a different number and must not be allowed to flatter the first.
  if (withRepair && !offline && r.root) {
    const before = repairableDiagnostics(r.diagnostics, SEVERITY, { includeAdvisory: true });
    if (before.length) {
      repairs.offered++;
      repairs.before += before.length;
      try {
        const patch = await generate(buildRepairPrompt(before),
          { currentSurface: text, sessionId: sessionFor(kase.id), model: modelKey });
        // The same seeding the runtime does: the delta overwrites by name, so
        // the prior surface has to be underneath it or `root` goes missing.
        const merged = `${text}\n${patch.text}`;
        const after = repairableDiagnostics(
          evaluateGeneration(merged).diagnostics, SEVERITY, { includeAdvisory: true });
        repairs.after += after.length;
        if (!after.length) { repairs.cleared++; console.log(`    repair: ${before.length} → 0`); }
        else if (after.length < before.length) { repairs.improved++; console.log(`    repair: ${before.length} → ${after.length}`); }
        else { repairs.noBetter++; console.log(`    repair: ${before.length} → ${after.length}, no better`); }
      } catch (err) {
        repairs.after += before.length;
        repairs.noBetter++;
        console.log(`    repair: failed — ${err.message}`);
      }
    }
  }

  if (kase.knownFailure) {
    if (gating.length) {
      console.log(`~ ${kase.id} — known failure: ${kase.knownFailure}`);
      for (const f of gating) console.log(`    ${f}`);
      // Printed here as well as below, because `continue` skips the tail. A
      // known-failing case was the one place an advisory was collected and
      // then thrown away — and it is the case most likely to be carrying a
      // second, unrelated mistake nobody has looked at yet.
      for (const a of advisory) console.log(`    ${a.includes('eval/') ? 'noted' : 'corrected'}: ${a}`);
      for (const t of runtime) console.log(`    runtime: ${t}`);
    } else {
      // A known failure that passes is a fix nobody wrote down. Failing here is
      // what stops the annotation outliving the problem and quietly hiding a
      // real regression later.
      failed++;
      console.error(`✗ ${kase.id} — marked as a known failure but it passes now.`);
      console.error(`    Remove knownFailure: "${kase.knownFailure}"`);
    }
    continue;
  }

  if (gating.length) {
    failed++;
    console.error(`✗ ${kase.id} — "${kase.prompt}"`);
    for (const f of gating) console.error(`    ${f}`);
  } else {
    const shape = r.root
      ? `${r.statements} statements, ${r.queries.length} queries, ${r.tags.length} components`
      : 'prose only';
    console.log(`✓ ${kase.id} — ${shape}`);
  }
  // On a replay these are not in `gating`, so they would otherwise vanish —
  // and a finding nobody prints is a finding nobody fixes. Marked `quality:`
  // so the line says which verdict it belongs to without anyone having to
  // remember which mode they ran in.
  if (offline) for (const q of quality) console.log(`    quality: ${q}`);
  for (const a of advisory) console.log(`    ${a.includes('eval/') ? 'noted' : 'corrected'}: ${a}`);
  // Four independent readings, printed as four. A ✓ above means the surface
  // is sound; these say which mechanism it chose, and the two can and do
  // disagree — a table that pages client-side over one fetched page renders
  // perfectly and cannot reach row 21.
  if (dimensions) {
    const mark = (ok) => (ok ? '✓' : '✗');
    console.log(`    mechanism: ${mark(dimensions.sourceCorrect)} source`
      + `  ${mark(dimensions.paginationCorrect)} pagination`
      + `  ${mark(dimensions.searchMechanismCorrect)} search`
      + `  ${mark(dimensions.totalCorrect)} total`);
    const d = dimensions.detail;
    const step = (name, x) => (x.action ? `${name}=${x.action}${x.runs ? '' : ' (no @Run)'}` : `no ${name}`);
    const notes = [
      `chose ${d.sourceUsed.join('+') || 'no source'}`,
      d.pageArg ? `page<-${d.pageArg}` : 'page is a literal',
      `limit ${d.limitResolved ?? d.limitArg ?? 'unset'}${d.limitFrom ? ` (${d.limitFrom})` : ''}`,
      step('next', d.next), step('prev', d.prev),
      d.queryArg ? `query<-${d.queryArg}` : 'query is a literal',
      d.clientPagination ? 'app-table pages the fetched page' : null,
      d.clientFilter ? '@Filter over the fetched page' : null,
    ].filter(Boolean);
    console.log(`               ${notes.join('; ')}`);
  }
  if (controlPair) {
    const mark = (ok) => (ok ? '✓' : '✗');
    const d = controlPair.detail;
    console.log(`    pair: ${mark(controlPair.sourceCorrect)} source`
      + `  ${mark(controlPair.argumentCorrect)} argument`
      + `  ${mark(controlPair.controlKindCorrect)} control`
      + `  ${mark(controlPair.workingRefetchCorrect)} working refetch`);
    const notes = [
      `chose ${d.sourceUsed.join('+') || 'no source'}`,
      `month<-${d.argRaw ?? 'unset'}`,
      `control ${d.controlUsed ?? 'none'}`,
      d.setter ? `setter ${d.setter}${d.actionInSlot === d.setter ? '' : ' (NOT in the action slot)'}` : 'no setter',
      d.setterRunsQuery ? 'refetches' : 'no @Run on it',
      d.positionalFindings.length ? `positional: ${d.positionalFindings.join('+')}` : null,
      d.runAuthoredUnreachable ? `${d.runAuthoredUnreachable} @Run(s) nothing can fire` : null,
      d.clientFilterOnDate ? '@Filter on "date"' : null,
    ].filter(Boolean);
    console.log(`          ${notes.join('; ')}`);
  }
  if (searchNaming) {
    const n = searchNaming;
    const mark = (ok) => (ok ? '✓' : '✗');
    console.log(`    search state: ${n.appSearchStateName ?? 'none'}`
      + `  [${n.appSearchStateNamingClass}]`
      + `  ${mark(n.appSearchStateMatchesQueryArgument)} name is a query argument`
      + `  ${mark(n.appSearchStateBoundToAnyQueryArgument)} reaches some argument`
      + `  ${mark(n.appSearchStateBoundToIntendedQueryArgument)} reaches the intended one`);
  }
  // Offline, nothing should reach the network or the stream. One of these
  // means the harness, not the generation.
  for (const t of runtime) console.log(`    runtime: ${t}`);
}

const known = cases.filter((c) => c.knownFailure).length;
const mode = offline ? 'replayed' : 'live';

// ── The run's own record ────────────────────────────────────────────────────
// Written before any of the reporting below, so a run that exits non-zero
// still leaves its evidence behind. Every number here is observational: this
// file decides nothing, and nothing reads it to gate.
if (record && runDir) {
  const kinds = new Set(Object.values(runRows).flatMap((c) => c.uniqueKinds ?? []));
  const diagnostics = {};
  let fatal = 0;
  for (const c of Object.values(runRows)) {
    for (const [code, n] of Object.entries(c.diagnostics ?? {})) {
      diagnostics[code] = (diagnostics[code] ?? 0) + n;
    }
    fatal += c.fatalDiagnostics ?? 0;
  }
  const { generator, generatorConsistent, generatorSpread } = runProvenance(caseProvenance);
  const summary = {
    _generated: 'by ui/scripts/eval-generations.mjs --record — observational, gates nothing',
    timestamp: new Date().toISOString(),
    catalogVersion: catalog.catalogVersion,
    manifestVersion: MANIFEST.manifestVersion ?? null,
    // What produced these generations. `generatorConsistent: false` means a
    // redeploy landed mid-run and the cases below were not all written by the
    // same generator — the run is still readable case by case, and it is not
    // one experimental condition.
    generator,
    generatorConsistent,
    ...(generatorConsistent ? {} : { generatorSpread }),
    mode: withRepair ? 'record+repair' : 'record',
    snapshotDir: runDir.slice(runDir.indexOf('ui/')),
    caseCount: cases.length,
    corpus: {
      uniqueKinds: [...kinds].sort(),
      uniqueKindCount: kinds.size,
      catalogSize: CATALOG_SIZE,
      breadth: `${kinds.size}/${CATALOG_SIZE}`,
      fatalDiagnostics: fatal,
      diagnosticCounts: diagnostics,
      skipped, unreachable,
      // Counts only. No rate, no test, no verdict — the association behind
      // them is observational and the direction of the arrow is unknown.
      appSearchNaming: (() => {
        const n = Object.values(runRows).map((c) => c.searchNaming).filter(Boolean);
        if (!n.length) return null;
        const m = n.filter((x) => x.appSearchStateMatchesQueryArgument);
        const byClass = {};
        for (const x of n) byClass[x.appSearchStateNamingClass] = (byClass[x.appSearchStateNamingClass] ?? 0) + 1;
        return {
          total: n.length,
          exactNameMatches: m.length,
          exactNameMatchesBindingAnyArgument: m.filter((x) => x.appSearchStateBoundToAnyQueryArgument).length,
          exactNameMatchesBindingIntendedArgument: m.filter((x) => x.appSearchStateBoundToIntendedQueryArgument).length,
          nonMatchesBindingAnyArgument: n.filter(
            (x) => !x.appSearchStateMatchesQueryArgument && x.appSearchStateBoundToAnyQueryArgument).length,
          byNamingClass: byClass,
        };
      })(),
    },
    cases: runRows,
  };
  writeFileSync(resolve(runDir, 'summary.json'), `${JSON.stringify(summary, null, 2)}\n`);
  console.log(`\nrun saved: ${summary.snapshotDir}`);
  if (summary.generator) {
    console.log(`  generator: prompt ${summary.generator.promptDigest ?? '?'}`
      + `  spec ${summary.generator.specDigest ?? '?'}`
      + `  code ${summary.generator.generatorDigest ?? '?'}`
      + `  model ${summary.generator.model ?? '?'}`);
  } else if (!offline) {
    console.log('  generator: no provenance reported — this generator predates it, '
      + 'so this run cannot be attributed to a prompt revision');
  }
  if (!summary.generatorConsistent) {
    console.error('\neval: THIS RUN IS NOT ONE EXPERIMENT. Cases in it report different '
      + 'promptDigest values, which means the generator was redeployed while it ran:');
    for (const [digest, ids] of Object.entries(summary.generatorSpread ?? {})) {
      console.error(`  ${digest}: ${ids.join(', ')}`);
    }
    console.error('The .dsl files are kept — each is still a real generation — but do not '
      + 'compare this run against another as a condition.');
  }
  console.log(`  breadth ${summary.corpus.breadth} unique component kinds`
    + `   fatal diagnostics: ${fatal}`);
  console.log('  compare a later run with:'
    + `\n    node ui/scripts/eval-generations.mjs --compare ${summary.snapshotDir}`);
}

// The one number that says whether handing diagnostics back is worth the
// round trip. Printed before the pass/fail line because it is about a
// different thing: not how good the first draft was, but how much of its own
// mess the generator can clear once it is told.
if (withRepair && !offline) {
  if (!repairs.offered) {
    console.log('\nrepair: nothing to repair — no generation carried a fixable diagnostic.');
  } else {
    console.log(`\nrepair: ${repairs.offered}/${cases.length} generations needed one; `
      + `${repairs.cleared} fully cleared, ${repairs.improved} improved, ${repairs.noBetter} no better.`);
    console.log(`        ${repairs.before} fixable diagnostics → ${repairs.after} `
      + `(${Math.round((1 - repairs.after / repairs.before) * 100)}% cleared in one turn).`);
    if (repairs.noBetter > repairs.cleared + repairs.improved) {
      console.log('        More turns wasted than helped. In the browser those roll back, so');
      console.log('        the cost is latency rather than a worse dashboard — but at this rate');
      console.log('        the repair prompt is the thing to look at, not the round count.');
    }
  }
}

// Recording captures; replaying judges. Generation is stochastic, so each
// record run samples the distribution afresh and will surface different
// things — failing the recipe on a sample turns that into whack-a-mole, and
// worse, it stops before you can look at what it just wrote. What it wrote is
// the point. The committed fixtures are what gate CI, through --offline.
// Every case answering nothing is not a bad sample, it is a broken setup —
// the generator is not generating. Said separately and first, because the
// per-case failures underneath all read as "the model wrote nothing useful"
// and none of them names the actual cause.
if (record && unreachable.length === cases.length) {
  console.error(`\neval: all ${cases.length} requests failed — nothing was recorded, your fixtures are untouched.`);
  console.error('Every case got the same answer from the control plane, so this is the route');
  console.error('or the request, not the model. The first ✗ line above carries what the');
  console.error('server said; a 400 names the field it rejected, a 503 means no running agent.\n');
  process.exit(1);
}

if (record && skipped.length === cases.length) {
  console.error(`\neval: all ${cases.length} came back empty — nothing was recorded, your fixtures are untouched.`);
  console.error('The generator answered nothing at all, including the prose-only cases, which');
  console.error('touch neither the catalog nor the DSL. That is a setup problem, not a model one.\n');
  console.error('Weave runs as an agent the control plane deploys, so its environment comes');
  console.error('from the server env file by way of oss/server/src/seed.rs — not from any file in');
  console.error('the Weave repo. Check, in this order:\n');
  console.error('  1. docker logs $(docker ps -qf name=weave) — the agent says why on the');
  console.error('     first request, and it is usually a model credential.');
  console.error('  2. AWS_BEARER_TOKEN_BEDROCK in the server env file — the seed forwards it,');
  console.error('     so a stale or rotated key here is silence there.');
  console.error('  3. WEAVE_FORCE_PULL=1 then restart, if the env changed since the agent');
  console.error('     was last deployed — a running container keeps the env it started with.');
  process.exit(1);
}

if (record && skipped.length) {
  console.error(`\neval: ${skipped.length} answered nothing and were NOT recorded — ${skipped.join(', ')}.`);
  console.error('Those fixtures keep whatever they had. Re-run them once the cause is fixed.');
}

if (record && unreachable.length) {
  console.error(`\neval: ${unreachable.length} never reached the generator — ${unreachable.join(', ')}.`);
  console.error('Those fixtures were not touched.');
}

if (record && failed) {
  const wrote = cases.length - skipped.length - unreachable.length;
  console.error(`\neval: recorded ${wrote}; ${failed} would fail as a baseline.`);
  console.error('Read them, then either fix the cause or re-record for a different sample.');
  console.error('CI judges the committed fixtures — run --offline before you commit.');
  process.exit(0);
}

if (failed) {
  console.error(`\neval: ${failed} of ${cases.length} failed (${mode}).`);
  process.exit(1);
}
console.log(
  `\neval: ${cases.length} generations (${mode}), catalog ${catalog.catalogVersion}`
  + (known ? ` — ${known} known failure(s), see knownFailure in CASES.` : ', all renderable.'),
);

}
