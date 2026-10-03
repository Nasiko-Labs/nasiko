import { render, screen } from '@testing-library/react'
import { Box } from 'lucide-react'
import { describe, expect, it } from 'vitest'
import { deferred } from './deferred'
import { applyNav, OSS_SLOTS, resolveSlots, type EditionLayer, type GranteeKind } from './edition'
import { useEdition, useSlots } from './edition-context'
import { EditionProvider } from './EditionProvider'
import { NAV_ITEMS } from './shell/nav'

const kind = (key: string): GranteeKind => ({ key, label: key, Section: () => null })

describe('resolveSlots', () => {
  it('is the OSS fallback with no layers', () => {
    expect(resolveSlots([])).toEqual(OSS_SLOTS)
  })

  it('lets the upper layer win a scalar slot and concatenates array slots, lowest first', () => {
    const A = () => null
    const B = () => null
    const ee: EditionLayer = { id: 'ee', slots: { topbarStart: A, granteeKinds: [kind('units')] } }
    const mt: EditionLayer = {
      id: 'mt',
      slots: { topbarStart: B, granteeKinds: [kind('workspaces')] },
    }
    const s = resolveSlots([ee, mt])
    expect(s.topbarStart).toBe(B)
    expect(s.granteeKinds.map((k) => k.key)).toEqual(['units', 'workspaces'])
    expect(s.harnessOrgScope).toBeNull()
  })
})

describe('applyNav', () => {
  const item = { label: 'Users', icon: Box, group: 'work' as const, shared: false }
  it('inserts after a core item, hides by url and appends to a group; the core list is never redeclared', () => {
    const layer: EditionLayer = {
      id: 'ee',
      nav: [
        { op: 'insert', after: '/agents', items: [{ ...item, to: '/users' }] },
        { op: 'hide', urls: ['/builds'] },
        { op: 'append', group: 'lab', items: [{ ...item, to: '/custom-views' }] },
      ],
    }
    const urls = applyNav(NAV_ITEMS, [layer]).map((i) => i.to)
    expect(urls.slice(0, 4)).toEqual(['/', '/chat', '/agents', '/users'])
    expect(urls).not.toContain('/builds')
    expect(urls.at(-1)).toBe('/custom-views')
    expect(applyNav(NAV_ITEMS, [])).toEqual(NAV_ITEMS)
  })
})

describe('EditionProvider', () => {
  function Probe() {
    return <p>{`${useEdition()}:${useSlots().granteeKinds.length}`}</p>
  }
  it('defaults to OSS, and hands pages the edition and its resolved slots', () => {
    const { unmount } = render(<Probe />)
    expect(screen.getByText('oss:0')).toBeInTheDocument()
    unmount()
    render(
      <EditionProvider
        edition={{ id: 'ee', layers: [{ id: 'ee', slots: { granteeKinds: [kind('units')] } }] }}
      >
        <Probe />
      </EditionProvider>,
    )
    expect(screen.getByText('ee:1')).toBeInTheDocument()
  })

  it('deferred() renders the slot once its chunk arrives', async () => {
    const Slot = deferred(async () => ({ name }: { name: string }) => <span>hello {name}</span>)
    render(<Slot name="ee" />)
    expect(await screen.findByText('hello ee')).toBeInTheDocument()
  })
})
