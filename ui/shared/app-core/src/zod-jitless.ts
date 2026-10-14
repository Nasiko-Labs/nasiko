// The content security policy forbids `eval` in the browser (no 'unsafe-eval' in a production script-src, contract
// §17.5). Zod 4 compiles object parsers with `new Function` when it may, and its feature probe alone is reported as a
// CSP violation. Every workspace app turns the compiler off once, before any schema parses (zod's own `jitless`
// switch; parsing stays correct, only the compiled fast path is skipped). Imported for its effect as the first
// import of each app's `main.tsx` (so it runs before application code) and, as a fallback, by the edition provider.
// From `zod/mini`, which the shell's search schemas already load: the switch is zod's one global config (shared by
// `zod` and `zod/mini`), and importing it from full `zod` put all of classic zod in every route's first load. Through
// the namespace import, as everywhere (a named or `zod/v4/core` import keeps all 61 zod locale tables).
import * as z from "zod/mini";

z.config({ jitless: true });
