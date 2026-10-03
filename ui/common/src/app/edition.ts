/**
 * How an edition builds on the core (docs/lab-vs-react-migration-review.md §10.4). Pure TypeScript, no React:
 * the glue is edition-context.tsx.
 *
 * - The EDITION is fixed at build time: each edition's `src/edition.ts` names its id and its layers, and nothing
 *   checks it at run time except `useEdition()`. An OSS build contains no EE code at all (the build's leak guard).
 * - A layer changes the core only through `slots` (each has an OSS fallback in the core) and `nav` patches
 *   relative to core items; it never redeclares the item list, so a new core item shows in every edition.
 * - Capability (what this server can do right now) is still probed at run time, and only for features both
 *   editions have (the harness usage endpoint's bare vs coded 404, crash-guardian fields). A probe never stands in
 *   for the edition.
 */
import type { ComponentType } from 'react'
import type { NavGroupId, NavItem } from './shell/nav'

export type EditionId = 'oss' | 'ee' | 'mt'

/** What the Harnesses page may show above the individual view. OSS: only the viewer's own usage. */
interface HarnessOrgScope {
  /** The org-level page (units, landing rules, org/team rows, the breadcrumb root). */
  Page: ComponentType
}

/** Who else an agent can be shared with, besides users (EE: org units). */
export interface GranteeKind {
  key: string
  label: string
  /** The Access tab section for this kind of grantee. */
  Section: ComponentType<{ agentId: string }>
}

/** Who else an MCP server can be shared with, besides users (EE: org units). */
interface ConnectorGranteeKind {
  key: string
  label: string
  /** The server page's Access tab section for this kind of grantee. */
  Section: ComponentType<{ connectorId: string }>
}

/** A section a layer adds to Settings' in-page nav (EE: Single sign-on), at `/settings?section=<key>`. */
export interface SettingsSection {
  key: string
  label: string
  /** Which nav group it sits in; Security sections come before Secrets. */
  group: 'workspace' | 'security'
  /** The row it follows in its group (e.g. `general`); the end of the group when absent. */
  after?: string
  /** Admin-only, like the core's workspace sections. It draws its own page header (its words load with it). */
  Section: ComponentType
}

/** The org-unit filter a layer can add to TokenOps (planned in EE). */
interface OrgUnitFilterProps {
  value?: string
  onChange: (orgUnit: string | undefined) => void
}

/** Every slot has an OSS fallback in the core; a slot is added in the PR whose layer first fills it. */
export interface Slots {
  harnessOrgScope: HarnessOrgScope | null
  granteeKinds: GranteeKind[]
  connectorGranteeKinds: ConnectorGranteeKind[]
  settingsSections: SettingsSection[]
  tokenopsOrgUnitFilter: ComponentType<OrgUnitFilterProps> | null
  topbarStart: ComponentType | null
  shellEnd: ComponentType<{ path: string }> | null
}

/** The core's answer for every slot: an OSS build renders exactly this. */
export const OSS_SLOTS: Slots = {
  harnessOrgScope: null,
  granteeKinds: [],
  connectorGranteeKinds: [],
  settingsSections: [],
  tokenopsOrgUnitFilter: null,
  topbarStart: null,
  shellEnd: null,
}

/** A layer's own sidebar item: any path its routes serve. */
type LayerNavItem = Omit<NavItem, 'to'> & { to: string }
export type AnyNavItem = NavItem | LayerNavItem

type NavPatch =
  | { op: 'insert'; after: string; items: LayerNavItem[] }
  | { op: 'hide'; urls: string[] }
  | { op: 'append'; group: NavGroupId; items: LayerNavItem[] }

export interface EditionLayer {
  id: Exclude<EditionId, 'oss'>
  slots?: Partial<Slots>
  nav?: NavPatch[]
}

export interface Edition {
  id: EditionId
  layers: readonly EditionLayer[]
}

export const OSS: Edition = { id: 'oss', layers: [] }

/** Scalar slots: the upper layer wins. Array slots: concatenated, lowest layer first. */
export function resolveSlots(layers: readonly EditionLayer[]): Slots {
  const out: Slots = {
    ...OSS_SLOTS,
    granteeKinds: [],
    connectorGranteeKinds: [],
    settingsSections: [],
  }
  for (const layer of layers) {
    const s = layer.slots ?? {}
    if (s.granteeKinds) out.granteeKinds = [...out.granteeKinds, ...s.granteeKinds]
    if (s.connectorGranteeKinds)
      out.connectorGranteeKinds = [...out.connectorGranteeKinds, ...s.connectorGranteeKinds]
    if (s.settingsSections) out.settingsSections = [...out.settingsSections, ...s.settingsSections]
    if (s.harnessOrgScope !== undefined) out.harnessOrgScope = s.harnessOrgScope
    if (s.tokenopsOrgUnitFilter !== undefined) out.tokenopsOrgUnitFilter = s.tokenopsOrgUnitFilter
    if (s.topbarStart !== undefined) out.topbarStart = s.topbarStart
    if (s.shellEnd !== undefined) out.shellEnd = s.shellEnd
  }
  return out
}

/** The sidebar items after every layer's patches, in layer order. An unknown `after` appends to that item's group. */
export function applyNav(
  base: readonly AnyNavItem[],
  layers: readonly EditionLayer[],
): AnyNavItem[] {
  let items = [...base]
  for (const patch of layers.flatMap((l) => l.nav ?? [])) {
    if (patch.op === 'hide') items = items.filter((i) => !patch.urls.includes(i.to))
    else if (patch.op === 'append')
      items = [...items, ...patch.items.map((i) => ({ ...i, group: patch.group }))]
    else {
      const at = items.findIndex((i) => i.to === patch.after)
      items =
        at < 0
          ? [...items, ...patch.items]
          : [...items.slice(0, at + 1), ...patch.items, ...items.slice(at + 1)]
    }
  }
  return items
}
