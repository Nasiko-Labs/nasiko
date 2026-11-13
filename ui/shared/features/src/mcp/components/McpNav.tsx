/** MCP servers · Builds — the module's sub-pages, as Agents does with AgentsNav. */
import { SectionNav, type Section } from '@nasiko/app-core/shell/section-nav'
import { copy } from '../copy'

const SECTIONS = [
  { to: '/mcp', label: copy.title },
  { to: '/mcp/builds', label: copy.sections.builds },
] as const satisfies readonly Section[]

export function McpNav({ current }: { current: (typeof SECTIONS)[number]['to'] }) {
  return <SectionNav label={copy.sections.label} sections={SECTIONS} current={current} />
}
