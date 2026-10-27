import { QueryClientProvider } from '@tanstack/react-query'
import { createMemoryHistory, RouterProvider, type AnyRouter } from '@tanstack/react-router'
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { LazyMotion, domMax } from 'motion/react'
import { useEffect, useState, type ComponentType, type ReactNode } from 'react'
import { describe, expect, it } from 'vitest'
import { EditionProvider } from '@nasiko/app-core/edition/EditionProvider'
import { OSS, OSS_SLOTS, resolveSlots, type Edition } from '@nasiko/app-core/edition/edition'
import { createAppRouter } from '@nasiko/app-shell/router'
import { TooltipProvider } from '@nasiko/ui/primitives/tooltip'
import { createQueryClient } from '@nasiko/app-core/queryClient'
import { setupPinnedSeed } from '@nasiko/testing/pinnedSeed'

setupPinnedSeed()

const T = { timeout: 12_000 }

function mountApp(url: string, edition: Edition) {
  const routerRef: { current?: AnyRouter } = {}
  const queryClient = createQueryClient(() => routerRef.current, { retry: false })
  const router = createAppRouter({
    queryClient,
    history: createMemoryHistory({ initialEntries: [url] }),
  })
  routerRef.current = router as unknown as AnyRouter
  render(
    <EditionProvider edition={edition}>
      <QueryClientProvider client={queryClient}>
        <TooltipProvider>
          <LazyMotion features={domMax}>
            <RouterProvider router={router} />
          </LazyMotion>
        </TooltipProvider>
      </QueryClientProvider>
    </EditionProvider>,
  )
  return { router, queryClient }
}

const withSplit = (
  shellSplit: ComponentType<{ path: string; children: ReactNode }> | null,
): Edition => ({
  id: 'ee',
  layers: [{ id: 'ee', slots: { shellSplit } }],
})

const mounts = { split: 0, unmounts: 0 }

function PanelSplit({ path, children }: { path: string; children: ReactNode }) {
  const [open, setOpen] = useState(false)
  useEffect(() => {
    mounts.split += 1
    return () => {
      mounts.unmounts += 1
    }
  }, [])
  return (
    <div data-testid="split" data-path={path}>
      <div data-testid="page-column">{children}</div>
      <button type="button" onClick={() => setOpen((o) => !o)}>
        toggle panel
      </button>
      {open ? <aside aria-label="test panel" /> : null}
    </div>
  )
}

function RemountingSplit({ children }: { path: string; children: ReactNode }) {
  const [open, setOpen] = useState(false)
  return (
    <>
      <button type="button" onClick={() => setOpen((o) => !o)}>
        toggle panel
      </button>
      {open ? (
        <div>
          {children}
          <aside aria-label="test panel" />
        </div>
      ) : (
        children
      )}
    </>
  )
}

const wrapperOf = () => document.querySelector<HTMLElement>('[data-slot="sidebar-wrapper"]')!
const mainEl = () => document.getElementById('main')!

async function ready() {
  await screen.findByRole('navigation', { name: 'Main' }, T)
  await waitFor(() => expect(document.getElementById('main')).not.toBeNull())
}

const normalise = (html: string) =>
  html
    .replace(/radix-[^"\s]*/g, 'radix')
    .replace(/:r[0-9a-z]+:/g, 'id')
    .replace(/_r_[0-9a-z]+_/g, 'id')

function shellChrome() {
  const clone = wrapperOf().cloneNode(true) as HTMLElement
  const main = clone.querySelector('#main')
  main?.replaceChildren()
  return normalise(clone.outerHTML)
}

describe('the slot default', () => {
  it('is empty in the OSS slots and for an edition with no layers', () => {
    expect(OSS_SLOTS.shellSplit).toBeNull()
    expect(resolveSlots(OSS.layers).shellSplit).toBeNull()
  })

  it('a layer that does not name it leaves it empty', () => {
    expect(resolveSlots([{ id: 'ee', slots: {} }]).shellSplit).toBeNull()
  })

  it('a layer fills it, and a later layer wins', () => {
    const first = () => null
    const second = () => null
    expect(resolveSlots([{ id: 'ee', slots: { shellSplit: first } }]).shellSplit).toBe(first)
    expect(
      resolveSlots([
        { id: 'ee', slots: { shellSplit: first } },
        { id: 'mt', slots: { shellSplit: second } },
      ]).shellSplit,
    ).toBe(second)
  })

  it('a later layer that names it as null clears it', () => {
    const first = () => null
    expect(
      resolveSlots([
        { id: 'ee', slots: { shellSplit: first } },
        { id: 'mt', slots: { shellSplit: null } },
      ]).shellSplit,
    ).toBeNull()
  })
})

describe('with no slot (OSS)', () => {
  it('puts the page area directly in the shell: no wrapper between the sidebar wrapper and the page', async () => {
    mountApp('/account/appearance', OSS)
    await ready()
    expect(mainEl().parentElement).toBe(wrapperOf())
    expect(document.querySelector('[data-testid="split"]')).toBeNull()
  })

  it('renders the same shell as an edition that names the slot as null', async () => {
    mountApp('/account/appearance', OSS)
    await ready()
    const plain = shellChrome()
    cleanup()
    mountApp('/account/appearance', {
      id: 'oss',
      layers: [{ id: 'ee', slots: { shellSplit: null } }],
    })
    await ready()
    expect(shellChrome()).toBe(plain)
  })

  it('has the sidebar, then the page, with nothing in between', async () => {
    mountApp('/account/appearance', OSS)
    await ready()
    const kids = [...wrapperOf().children].map((c) => c.getAttribute('data-slot') ?? c.tagName)
    const at = kids.indexOf(mainEl().getAttribute('data-slot') ?? 'MAIN')
    expect(at).toBeGreaterThan(0)
    expect(mainEl().parentElement).toBe(wrapperOf())
  })

  it('has no launcher or panel of any kind', async () => {
    mountApp('/account/appearance', OSS)
    await ready()
    expect(screen.queryByRole('complementary', { name: 'Weave' })).toBeNull()
    expect(screen.queryByRole('button', { name: /weave/i })).toBeNull()
  })

  it('renders the same on a page that fills the viewport', async () => {
    mountApp('/settings', OSS)
    await ready()
    expect(mainEl().parentElement).toBe(wrapperOf())
  })
})

describe('with a slot filled (EE)', () => {
  const edition = withSplit(PanelSplit)

  it('wraps only the page area: the sidebar stays outside, the page inside', async () => {
    mountApp('/account/appearance', edition)
    await ready()
    const split = screen.getByTestId('split')
    expect(split.contains(mainEl())).toBe(true)
    expect(split.contains(screen.getByRole('navigation', { name: 'Main' }))).toBe(false)
    expect(split.parentElement).toBe(wrapperOf())
  })

  it('hands the slot the current path, and follows navigation', async () => {
    const app = mountApp('/account/appearance', edition)
    await ready()
    expect(screen.getByTestId('split')).toHaveAttribute('data-path', '/account/appearance')
    await act(() => app.router.navigate({ to: '/account/password' }))
    await waitFor(() =>
      expect(screen.getByTestId('split')).toHaveAttribute('data-path', '/account/password'),
    )
  })

  it('does not remount the page when the panel opens and closes: same nodes, same DOM state', async () => {
    mountApp('/account/appearance', edition)
    await ready()
    const main = mainEl()
    const column = screen.getByTestId('page-column')
    const content = main.querySelector('div')!
    content.setAttribute('data-probe', 'kept')
    await userEvent.click(screen.getByRole('button', { name: 'toggle panel' }))
    expect(await screen.findByRole('complementary', { name: 'test panel' })).toBeInTheDocument()
    expect(mainEl()).toBe(main)
    expect(screen.getByTestId('page-column')).toBe(column)
    expect(main.querySelector('div')).toBe(content)
    expect(content).toHaveAttribute('data-probe', 'kept')
    await userEvent.click(screen.getByRole('button', { name: 'toggle panel' }))
    expect(screen.queryByRole('complementary', { name: 'test panel' })).toBeNull()
    expect(mainEl()).toBe(main)
    expect(main.querySelector('div')).toBe(content)
    expect(content).toHaveAttribute('data-probe', 'kept')
  })

  it('keeps a page control focused and its typed value across a toggle', async () => {
    mountApp('/account/password', edition)
    await ready()
    const field = (await screen.findAllByLabelText(/password/i, {}, T)).find(
      (el) => el.tagName === 'INPUT',
    ) as HTMLInputElement
    await userEvent.type(field, 'secret-draft')
    await userEvent.click(screen.getByRole('button', { name: 'toggle panel' }))
    await userEvent.click(screen.getByRole('button', { name: 'toggle panel' }))
    expect(field).toBeInTheDocument()
    expect(field.value).toBe('secret-draft')
  })

  it('mounts the slot once and never remounts it across navigation', async () => {
    mounts.split = 0
    mounts.unmounts = 0
    const app = mountApp('/account/appearance', edition)
    await ready()
    await act(() => app.router.navigate({ to: '/account/password' }))
    await act(() => app.router.navigate({ to: '/settings' }))
    await waitFor(() =>
      expect(screen.getByTestId('split')).toHaveAttribute('data-path', '/settings'),
    )
    expect(mounts.split).toBe(1)
    expect(mounts.unmounts).toBe(0)
  })

  it('the page keeps working beside an open panel', async () => {
    mountApp('/account/appearance', edition)
    await ready()
    await userEvent.click(screen.getByRole('button', { name: 'toggle panel' }))
    expect(within(mainEl()).getAllByRole('heading').length).toBeGreaterThan(0)
  })
})

describe('the remount check itself', () => {
  it('fails for a slot that changes where it puts the page when it opens', async () => {
    mountApp('/account/appearance', withSplit(RemountingSplit))
    await ready()
    const main = mainEl()
    await userEvent.click(screen.getByRole('button', { name: 'toggle panel' }))
    await screen.findByRole('complementary', { name: 'test panel' })
    expect(mainEl()).not.toBe(main)
  })
})
