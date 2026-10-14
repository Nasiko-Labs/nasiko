// Ported from nasiko-ui-lab@1b99bdda common/src/features/chat/agentName.ts. Local edits: none beyond imports and
// Prettier.
/**
 * One agent, several spellings (v1b §2.1, G-3): `tool_call.agent` is the display form (the tool
 * name with `-`, space, `.`, `/` folded to `_`, then `_` → `-`), while `sub_status`/`sub_content`
 * carry the raw agent name. Comparing folded names treats them as one.
 */
export const foldAgentName = (name: string) =>
  name
    .trim()
    .toLowerCase()
    .replace(/[-_ ./]/g, "-");
