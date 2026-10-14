import * as z from "zod";

/** Display text from data: bounded, no control characters other than tab/newline. */
// eslint-disable-next-line no-control-regex -- the point of the rule
const CONTROL_CHARS = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/;
export const DisplayTextSchema = z
  .string()
  .max(4_000)
  .refine((s) => !CONTROL_CHARS.test(s), "control characters are not allowed");

/** A link offered by data (a row, a tool result). Rendered only when `isSafeHref` agrees. */
export const DataLinkSchema = z
  .object({
    href: z.string().max(2_048),
    label: DisplayTextSchema.max(200),
  })
  .strict();
export type DataLink = z.infer<typeof DataLinkSchema>;
