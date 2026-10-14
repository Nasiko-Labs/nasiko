// Ported from nasiko-ui-lab@1b99bdda common/src/test/setup.ts (the `server` it exports). Local edits: only the
// msw/node server, with no edition handlers and none of the lab's jsdom setup; tests call `listen`/`close`
// themselves.
import { setupServer } from "msw/node";
import { allHandlers } from "./handlers";

/** Every mock handler on Node's fetch, for Vitest. `onUnhandledFrame: "error"` makes a missing handler fail loudly. */
export const server = setupServer(...allHandlers);
