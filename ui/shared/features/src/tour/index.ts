// Ported from the legacy ui/ on development (3aac84f66) common/src/features/tour/index.ts (dd9fe0f64). Local edits:
// `deferred` from @nasiko/app-core; Prettier formatting.
/** The tour's lazy part: the overlay and every tour's text load in their own chunk, the first time a tour opens. */
import { deferred } from "@nasiko/app-core/deferred";

export const TourOverlay = deferred(() => import("./TourOverlay").then((m) => m.TourOverlay));
