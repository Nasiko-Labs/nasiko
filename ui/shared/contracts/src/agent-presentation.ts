import * as z from "zod";
import { DataLinkSchema, DisplayTextSchema } from "./content-schemas";

/**
 * Agent-interaction presentation contracts (contract §7 "build missing product
 * composites from approved primitives", §8.3 timeline / approval rows).
 *
 * These are the shapes the UI renders and NOTHING else: an approval card shows
 * a server-issued action request, an activity timeline shows events the backend
 * actually emitted. The UI never invents progress, confidence, provenance or
 * hidden reasoning. Field names are proposals for the Rust side (Phase 4
 * conversation transport); until then the synthetic fixtures emit them.
 */

export const ApprovalStateSchema = z.enum([
  "pending",
  "approved",
  "rejected",
  "expired",
  "executing",
  "failed",
]);
export type ApprovalState = z.infer<typeof ApprovalStateSchema>;

/** A parameter the agent proposes; rendered read-only, typed by `kind`. */
export const ApprovalParamSchema = z
  .object({
    name: DisplayTextSchema.max(80),
    kind: z.enum(["text", "number", "money", "secret", "link", "json"]),
    /** Already formatted by the server for `money`; raw otherwise. `secret` is redacted server-side. */
    value: DisplayTextSchema,
    unit: DisplayTextSchema.max(16).optional(),
  })
  .strict();

export const ApprovalRequestSchema = z
  .object({
    id: z.uuid(),
    /** Opaque server id of the proposed action; the UI echoes it back, never interprets it. */
    actionId: z.string().min(1).max(200),
    title: DisplayTextSchema.max(160),
    summary: DisplayTextSchema.max(1_000),
    tool: DisplayTextSchema.max(120),
    params: z.array(ApprovalParamSchema).max(32),
    /** Server-computed risk class; the UI maps it to a badge, nothing more. */
    risk: z.enum(["low", "medium", "high"]),
    state: ApprovalStateSchema,
    requestedAt: z.iso.datetime({ offset: true }),
    expiresAt: z.iso.datetime({ offset: true }).optional(),
    /** Who may decide; the UI disables the controls for anyone else. */
    decidableBy: z.array(z.uuid()).max(64).optional(),
    links: z.array(DataLinkSchema).max(8).optional(),
  })
  .strict();
export type ApprovalRequest = z.infer<typeof ApprovalRequestSchema>;

/** The only two things a user can send back; the server validates authority and state. */
export const ApprovalDecisionSchema = z
  .object({
    id: z.uuid(),
    actionId: z.string().min(1).max(200),
    decision: z.enum(["approve", "reject"]),
    reason: DisplayTextSchema.max(500).optional(),
  })
  .strict();
export type ApprovalDecision = z.infer<typeof ApprovalDecisionSchema>;

export const ActivityKindSchema = z.enum([
  "message",
  "tool.start",
  "tool.result",
  "tool.error",
  "approval.requested",
  "approval.decided",
  "status",
  "artifact",
]);
export type ActivityKind = z.infer<typeof ActivityKindSchema>;

export const ActivityEventSchema = z
  .object({
    id: z.string().min(1).max(200),
    kind: ActivityKindSchema,
    at: z.iso.datetime({ offset: true }),
    /** Concise explanation actually supplied by the backend; never synthesised client-side. */
    text: DisplayTextSchema.max(2_000),
    actor: z.enum(["agent", "user", "system", "tool"]),
    /** Server-declared status for `status`/`tool.*` events. */
    status: z.enum(["ok", "running", "warning", "error"]).optional(),
    /** Grouping key (a tool call id) so start/result pair up. */
    correlationId: z.string().max(200).optional(),
    links: z.array(DataLinkSchema).max(4).optional(),
  })
  .strict();
export type ActivityEvent = z.infer<typeof ActivityEventSchema>;
