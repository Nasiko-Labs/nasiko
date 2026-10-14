/** Client-safe limits (no zod). Schemas that enforce them import from here. */

/** A timeline shows the most recent N and offers "earlier"; it never renders unbounded history. */
export const ACTIVITY_PAGE_MAX = 200;
