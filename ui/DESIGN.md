# OpenRuntime UI Lab: design notes

What exists today, in one place (app-shell increment, `plans/feat-app-shell.md`, design review 8A; themes from
`docs/lab-vs-react-migration-review.md` §6.4, decided 2026-09-29). Values live in code: colours in `common/src/index.css`,
motion in `common/src/lib/motion.ts`, theme logic in `common/src/app/shell/theme.ts`.
`common/src/app/shell/design-doc.test.ts` fails if a token named here is missing from `index.css`.

## Colour tokens

E## Colour tokens

Every colour is a CSS variable in `common/src/index.css`, exposed to Tailwind through `@theme inline` (`bg-card`,
`text-muted-foreground`, ...). Never use literal colours in components.

| Token | Use |
|---|---|
| `--background`, `--foreground` | the page (white in light mode) |
| `--sidebar` | the sidebar surface: near-white with a hairline edge in light mode; between page and card in dark |
| `--card`, `--popover` (+ `-foreground`) | raised surfaces: white cards with hairline borders, light tooltips and menus |
| `--muted`, `--muted-foreground` | tracks, table heads, quiet fills; secondary text (≥ 4.5:1) |
| `--accent`, `--accent-foreground` | the theme's pale tint: the active nav item, badges, selection, menu focus |
| `--secondary` | maps to `--muted` |
| `--border`, `--input` | hairlines and field borders |
| `--primary`, `--primary-hover`, `--primary-foreground` | where you act: buttons, links, the active tab; hover is a designed deeper shade |
| `--primary-text` | accent-coloured text; equals `--primary`, which meets 4.5:1 as text in every theme |
| `--ring` | focus rings (≥ 3:1 on the page) |
| `--success`, `--warning`, `--destructive`, `--info` | feedback, also delta text (≥ 4.5:1 on cards); shared per mode, not per theme |
| `--logo` | the Nasiko mark only: yellow-600 `#BB8F06` light, yellow-200 `#F7E19C` dark |
| `--chart-1` … `--chart-5`, `--chart-1-edge` … `--chart-5-edge`, `--chart-other`, `--chart-other-edge` | chart series (below) |
| `--sidebar-*` | shadcn's sidebar tokens mapped onto the above; `--sidebar-accent` is the neutral hover fill (`--muted`) |

## Themes and mode

- **Mode:** System (default), Light, Dark. `.dark` on `<html>`. Stored as `openruntime.theme`.
- **Theme:** Carbon (default), Teal, Indigo, and Plum. `data-theme` on `<html>`, stored as
  `openruntime.accent` (the retired accent presets read as Carbon; `indigo` keeps its name).
- **Carbon** (`data-theme="carbon"`) is shadcn's default new-york neutral palette: Mist in light mode, Carbon in
  dark. It keeps the defaults except where this app's rules need more: a solid focus ring needs 3:1, so light mode's
  ring is the default's 0.556 grey rather than 0.708; light mode's `--muted-foreground` is 0.54 rather than 0.556 (4.5:1 on the `--muted` hover fill) and its `--accent` is 0.94 rather than 0.97 (0.97 is `--muted`, the
  sidebar's hover, so the current nav item read as a hover); `--primary-hover` is added; status colours stay per mode; charts
  use Two-tone in Teal's order (the default chart set has a yellow series). Each theme is a whole token set: neutrals tinted toward the
  primary, the primary, its pale tint, the ring and the chart order. Dark surfaces are composed, not inverted:
  page < sidebar < card.
- Both are applied before first paint by the inline script in `index.html` (same keys as `theme.ts`, checked by a test).
- Settings → Appearance (`/settings/appearance`, also the account menu's Theme submenu) has a Mode radio group and a
  Theme radio group drawn as picture tiles: each mode is a small drawing of the app in that mode (System split on a
  diagonal; fixed `--preview-light-*` / `--preview-dark-*` tokens, the same in every theme and mode), each theme a nav
  tint and button in its light `--primary`. The radio stays in the tile, visually hidden; the chosen tile has a
  `--primary` ring, a check mark and its name in medium weight, so the choice never relies on colour alone.
- **Contrast (WCAG AA), every theme × mode, checked by `common/src/app/shell/contrast.test.ts`:** text ≥ 7:1 and secondary
  text ≥ 4.5:1 on cards; text on the primary and on its hover shade ≥ 4.5:1; accent text on page and cards ≥ 4.5:1;
  the active nav text on its tint ≥ 4.5:1; the ring ≥ 3:1; status colours ≥ 4.5:1; the dark logo ≥ 7:1.
- Colours never animate when the mode or theme changes.

## Brand rules

- **Yellow is the logo only.** The mark (`NasikoMark.tsx`, the website's "N") takes `text-logo`. Yellow is never a
  button, a link, text on white (2.98:1) or a chart series. The favicon keeps its fixed yellow-600 file.
- **Warning is orange, never yellow:** `#B94E00` light, `#F49752` dark.
- **No ink:** no near-black chart series.
- **The primary appears only where you act:** its 600 shade on buttons, links and the active tab; its pale tint behind
  the active nav item and badges.

## Charts

- One palette, **Two-tone** (high-contrast pastels): cornflower `#4777D2`, peach `#FFB98C` (edge `#D95800`), orchid
  `#CD5EA2`, mint `#8FEDD0` (edge `#009F7A`), lavender `#AAA0E6` (edge `#583DA6`); Other `#DDE2E4` (edge `#8A9396`),
  `#6B7375` in dark mode.
- **Order per theme:** a chart never leads with its theme's hue. Teal: cornflower, peach, orchid, mint, lavender.
  Indigo: peach, orchid, mint, cornflower, lavender. Plum: mint, cornflower, peach, orchid, lavender. Carbon
  (no hue of its own): Teal's order. Medium and light
  fills alternate in every order.
- Colour follows the entity (agent, workflow, harness) across the chart, legend, share bar and table.
- At most five series, then Other. Stacked segments have 2 px card-coloured gaps. Light mode draws a 2 px top edge in
  the series edge token, `--chart-1-edge` … `--chart-5-edge` (≥ 3:1 on white); dark mode turns edges off (edge = fill; fills are 4–13:1 on dark cards).
- Lines, dots, heat cells and thin bars draw in the edge token (a pastel fill is too light for them on white);
  `common/src/lib/chart.ts` (`seriesAt`, `OTHER_SERIES`) picks them; the visx kit (`common/src/components/charts/`) draws bars
  with `edge` and stacks with `stackGap={2}`; `components/shared/chart-marks.tsx` `Swatch` is the legend key.
- **Motion:** bars grow from the baseline, lines sweep left to right, scatter points spring in (staggered), the
  hovered column stays solid while the rest fade, and the tooltip follows with a spring. Under reduced motion every
  chart is drawn at rest.
- Every chart keeps a Table view; never show a value by colour alone.

## Type

- Hanken Grotesk Variable for UI (`--font-sans`) and JetBrains Mono Variable for figures and ids (`--font-mono`),
  self-hosted through `@fontsource-variable`, Latin subset. No font CDN. `index.html` preloads the sans file.
- Sizes in use: page titles `text-xl` semibold (`PageHeader`), panel titles `text-sm` semibold, body `text-sm`,
  captions `text-xs`. Named steps below that (in `common/src/index.css` `@theme`, never an arbitrary `text-[11px]`):
  `text-2xs` 11 px (sidebar group headers, uppercase medium with 0.04em tracking; keyboard hints; small badges),
  `text-3xs` 10 px (the MOCK DATA / LIVE badge), `text-4xs` 9 px and `text-5xs` 8 px (monograms, the collapsed rail's
  one-letter badge), `text-code` 13 px (mono ids and code), `text-lead` 17 px (the trace narrative).
- Numbers that line up (tables, KPIs, deltas) use `tabular-nums`.

e

- Tailwind's 4 px scale (Tailwind 4 takes any step, e.g. `h-55` = 220 px, so fixed heights need no `[px]`). Page
  area: `px-4 py-4`, `max-w-page` 1400 px (Chat and Settings fill the viewport instead; Settings centres a `max-w-3xl` column). Sheets: `max-w-sheet-lg` 720 px, `max-w-sheet` 560 px and
  `max-w-sheet-sm` 480 px (`cn()` knows these names, so they beat the primitive's `sm:max-w-sm`).
- Radius: `--radius` 0.5 rem (`rounded-lg`); `rounded-md` (6 px) for rows and controls.
- Touch targets: 32 px rows, 44 px on coarse pointers (`pointer-coarse:`).

## Motion

Presets in `common/src/lib/motion.ts`; new motion uses the names, never raw numbers. Everything sits under
`MotionConfig reducedMotion="user"`; CSS motion has `motion-reduce:` variants.

| Preset | Value | Used by |
|---|---|---|
| `fast` | 150 ms | menus, popovers, tooltips (tw-animate-css) |
| `base` / `collapse` | 180 ms ease-out | sidebar collapse (CSS width transition in `sidebar.tsx`) |
| `standard` / `disclosure` | 200 ms ease-out | disclosures (the shared `Disclosure` uses tw-animate-css's matching `collapsible-down/up`), row entrances, fades |
| `panelIn` / `panelOut` | 220 / 160 ms | sheets and side panels (`sheet.tsx`); reduced motion keeps the fade, drops the slide |
| `morph` | Motion's default | shared-layout `layoutId` morphs |
| `wordHold` | 2800 ms | the login showcase's headline word (the prototype's swap), each arriving on `--animate-word-in` (0.7 s blur-in); under reduced motion the first word stays |
| `--animate-glow` | 10 s loop (blobs at 10 / 11 / 12 / 14 s, out of phase) | the login showcase's glow: moves and cycles the four `--showcase-glow-*` hues over `--showcase-glow-base` (CSS); every user sets `motion-reduce:animate-none` |
| `--animate-beam` / `--animate-stage` | 4 s linear loop | the onboarding Welcome step's flow and the router's "How routing works": a beam crosses the stages and each icon lights as it passes (CSS); still under reduced motion, the first stage lit |
| `--animate-mark-draw` | 1.6 s ease-in-out, alternating | the page loader (`PageLoader`, our take on Aceternity's LoaderThree): each bar of the Nasiko mark draws its outline, then fills, 40 ms apart (CSS); reduced motion shows the filled mark, still |
| `--animate-float` | 7 s ease-in-out loop | the login showcase's layer stack bobs 8 px (CSS); still under reduced motion |
| `--animate-orbit` / `--animate-flag-wave` | 3 s linear / 1.4 s ease-in-out loops | the sidebar's waitlist badge (`WaitlistCta`, our take on Aceternity's Moving Border): a glint circles the pill's edge and the flag's cloth waves (CSS); under reduced motion the glint is hidden and the flag still |

`tw-animate-css` provides the `animate-in` / `slide-in-*` / `fade-*` utilities; a build test checks they are emitted.

## Sidebar

| Element | Spec |
|---|---|
| Width | 240 px expanded, 56 px icon rail; below 768 px a sheet from a slim top bar |
| Rows | 32 px (44 px on touch), 16 px lucide icons, 14 px labels |
| Header | the mark and the brand name (`copy.brand`), linking to the Overview, and the collapse button (⌘B / Ctrl+B) top right. In the rail the mark alone is the Expand button: hover or keyboard focus swaps it for the expand icon (150 ms fade, none under reduced motion), with the "Expand (⌘B)" tooltip |
| Groups | by job: Overview, Chat (no header); FLEET: Agents, Workflows; CONNECT: LLM router, MCP servers; OBSERVE: Sessions, TokenOps, Harnesses; LAB (development builds only): Weave fixtures. In the rail a group header becomes a 20 px `--sidebar-border` hairline (8 px above and below), so the icons stay one even column. Deploy and Builds are Agents' sub-pages (its `SectionNav`: All agents · Your agents · Builds) and mark Agents current |
| Active item | `--accent` tint fill, `--accent-foreground` label, medium weight, 6 px radius, `aria-current="page"`; hover on other rows is `--muted`, never the tint |
| Drill-in panel | a page with its own nav (Chat's history, Settings' sections) renders `SidebarPanel`: in the expanded sidebar and the phone sheet it replaces the nav groups under a row reading "Back" (its name and tooltip say where: "Back to TokenOps"): it returns to the last page visited outside the module (with its search; moves inside Chat or Settings don't count; repeated Backs keep going back, like history), or, when the page was opened directly ("Back to main menu"), shows the app nav until the next navigation; the collapsed rail keeps the nav icons and the page shows its own fallback beside it (Chat's rail column or sheet, Settings' section column). Never two sidebars side by side at full width |
| Footer | status (health + MOCK DATA / LIVE, links to Status at `/status`; dev servers and mock builds only), account (Theme submenu: Mode and Theme radios; Settings; Sign out) |
| Waitlist (OSS only) | above the footer, linking to the waitlist page (`env.waitlistUrl`): a `--card` card (8 px radius, hairline) with an "Early access" pill on its top edge (`--primary` fill, mono 10 px caps, a waving flag, a glint circling its edge: `--animate-orbit` / `--animate-flag-wave`), a title, one muted line and a full-width outline "Join the waitlist" that opens a new tab; in the rail one ticket-icon row with a tooltip. Not shown while a page's panel holds the sidebar (Chat, Settings), so Chat's dot background stays its one effect; no dismiss. The login page has the plain line "New to Nasiko? Join the waitlist" under Sign in |
| State | shadcn's `sidebar_state` cookie; first visit opens at ≥ 1280 px, on every page |

Add a nav item in `common/src/app/shell/nav.ts` (label, icon, route, group, `shared`, `also`) once its page exists.

## Chat surfaces (v1c)

| Element | Spec |
|---|---|
| Column | `CHAT_COLUMN` (`max-w-3xl`, 16 / 24 px sides): header content, turns and composer share one left edge |
| Composer | a card (`rounded-xl`, `--border`, `--card`, `shadow-xs`) with the textarea, the target chip, the key hint and a 32 px Send inside (44 px on touch) |
| Lifted panel | `LIFTED`: `--card` on the page background, `rounded-xl`, 1 px `--border`, `shadow-sm`; rows inside are separated by hairlines, never nested cards. Used by the new chat's target list and recent chats |
| Rail row | about 44 px, a plain row like the sidebar's own items (`RAIL_ROW` in `turnStyles.ts`: no box, `rounded-md`, 2 px gap): hover `--muted`, the open chat `--accent` with `--accent-foreground` on both lines; a 20 px identity icon, two lines (title; kind · time) and a fixed 16 px indicator slot so marks line up. Waiting rows use the same row |
| Identity icons | agent: monogram on `--muted`; Orchestrator: route icon on `--primary` at 10% with `--primary-text`; recorded: terminal icon on `--muted`; removed: crossed-out bot, muted |

Reference mockups: `plans/feat-chat-v1c.md` §5.17. The images set the look; the plan's text binds.

## Router surfaces

| Element | Spec |
|---|---|
| Page | Title and "How routing works" disclosure, four summary tiles (Your agents, Your default, Attached, No config: `--card` with a hairline, label, `text-2xl` figure, one-line note; each a filter toggle with `aria-pressed`, the pressed one with a `--primary` border and ring plus a check), then `Tabs` Agents · Configs · Providers (`?tab=`, replaced; one panel shown, the others mounted but `hidden`) |
| Agent rows | a `role="table"` grid on `--card` (six columns from 768 px, stacked below with the header kept `sr-only`); the attached config's name links to its row; Change routing is a ghost text button in `--primary-text` |
| KeySource chip | `--muted` fill, 1 px `--border`, `rounded-md`, 12 px text in `--muted-foreground`; key icon + "Your key · NAME" or building icon + "Platform key" (never colour alone) |
| Badges | outline `Badge`: Attached / Your default / Owner’s default / No config, and Custom; secondary `Badge`: "Default" on a config |
| Warnings | `Warn` (`components/bits.tsx`): 14 px `AlertTriangle` in `--warning` beside 12 px text; blocked clears add "Duplicate without this field" |
| Sheets | right side, full width below 640 px: config editor and custom provider 720 px, agent routing 560 px; sticky footer with the timing note; focus returns to the opener |
| Live region | one polite `role="status"` per page (`Announcer`): saves, deletes, moves, test and sync results. Section errors and outcome panels are plain text; only field errors inside a sheet use `role="alert"` |
| Your agents folding | agents that only follow your default fold into one `--muted`/30 summary row (chevron, "14 agents on your default", config · key, 30-day spend, "Show all 14" in `--primary-text`); attached, overridden, failed and just-changed rows stay visible; the choice is remembered; filters and the search (from 20 agents) unfold |
| How routing works | a card under the title: an uppercase `--muted-foreground` caption, five steps (32 px icon tiles, title with its mono `01`–`05`, one line) in a row from `lg` on a hairline a `--primary` beam travels along (`--animate-beam`), each tile lighting as it passes (`--animate-stage`); stacked below with a connecting line, the tiles still lighting in turn; steps rise in with the `enter` stagger; under reduced motion nothing moves and step 1 stays lit; a `--muted`/40 footer with the viewer's own numbers and "Show them"; its own close button; fades with `transitions.disclosure` |
| Budgets (R2) | a hairline `role="table"`: Scope, Used of limit (6 px bar in `--primary`, `--warning` or `--destructive`, then "$80.24 of $95.00"), Forecast, State badge with an icon, At 100%, text actions. Six columns from 1024 px (`lg`), stacked with inline labels below; the Alerts list is a separate hairline list under it |
| Budget sheet | 480 px; threshold chips (`--muted` pill, remove button per chip; with Stop calls the 100% chip is locked); the Stop-calls consequence inline in `--warning` with the triangle, tied to the radio with `aria-describedby`, never a dialog; removal confirms inline and moves focus into the confirmation |
| Nav icon | lucide `Waypoints` (the Orchestrator keeps the route icon) |

## Deploy surfaces

Stock shadcn only (plans/feat-deploy.md design review 14, 17): no custom components, only local composition.

| Element | Spec |
|---|---|
| Deploy page | `Tabs` for Upload a zip / From GitHub / From a registry (icons `Upload`, `FolderGit2`, `Container`); the form and the "What the zip needs" checklist sit side by side from 768 px of the `@container/deploy` wrapper, stacked below |
| Drop zone | `Empty` (dashed `--border`) with a "Choose file" `Button` over a visually hidden `Input type="file"`; drag-and-drop on the box is the only addition; 120 px tall on touch. The checklist marks each item met, missing or unknown in words, never colour alone |
| Stepper | one `Progress` per stage in a grid (as many columns as stages), 6 px tall on `--muted`: done fills `--primary`, the current stage half-fills with a reduced-motion-safe pulse, a failure fills `--destructive`, a built-but-not-running agent `--warning`. The current stage has `aria-current="step"`; below 520 px only its label shows ("Building image · step 2 of 4") |
| Status badges | `BuildBadge`, always a word: queued `--muted`, building `--info`, running and success `--success`, failed `--destructive`, not running `--warning`. Status colours never follow the accent |
| Build rows | the Builds list and an agent's Builds tab share one row: version link, badge, source, started, duration; below 768 px two lines with the headers kept `sr-only`. Failed rows add a one-line reason in `--muted-foreground`, never raw server text |
| Outcome strip | a `Card` under the stepper: "<name> is running" with Chat with it / Open agent, or the problem, its fix and one action (Deploy as vX, Deploy again) |
| Toasts | one Sonner `Toaster` in the app shell, bottom right, on `--popover` / `--popover-foreground` / `--border`. Only a build that finishes away from its page toasts, once, with one action (Chat with it, See why, Open agent). Pages keep their own live regions; the toast never repeats what the open page already says |
| Nav count | the Agents sidebar item (Builds is its sub-page) shows a `SidebarMenuBadge` with the number of builds this session still follows; its link names it ("Agents, 1 build in progress"); hidden in the icon rail |
| Entry points | "Deploy an agent" (`Upload` icon, `Button` size sm, 44 px on touch) in every CLI-only empty state and in the Agents and Your agents headers; the CLI steps stay under it as "Or use the CLI" |

## Focus and accessibility

- Focus is one style everywhere: a 2 px `--ring` with a 2 px offset in the surface colour (the page, or `--sidebar`
  for sidebar rows), on buttons, links, fields, selects, tabs, toggles, checkboxes, radios, switches and badges.
- The sidebar is `<nav aria-label="Main">`; labels stay in the DOM when collapsed, so names never depend on tooltips.
- A "Skip to content" button comes first (a button, not an `#main` link: a fragment would add a history entry and make
  the router reload the route).
- Selection is never shown by colour alone (theme swatches carry names and a radio indicator).
- One live region for chat announcements (`StatusAnnouncer`); don't add `role="alert"` there.
