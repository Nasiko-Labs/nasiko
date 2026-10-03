/** Fixes from the /ship pre-landing review (2026-09-30): each case failed before its fix. */
import { QueryClient } from '@tanstack/react-query'
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { server } from '@/test/setup'
import { copy } from './copy'
import { followedCount, setNotifier } from './follower'
import { clearUploads, clonedRepo, importFor, startImport } from './uploads'

setupPinnedSeed()
afterEach(() => {
  clearUploads()
  configureMocks({ seed, now, loggedIn: true, variant: null, superuser: null })
})

const pick = async (fullName: string) => {
  const rows = await screen.findAllByTestId('repo-row', {}, { timeout: 5000 })
  await userEvent.click(
    within(rows.find((r) => r.textContent?.includes(fullName))!).getByRole('radio'),
  )
}

describe('/ship review fixes', () => {
  it('forgets an import once its toast announced it, so the Registry tab never jumps to it later', async () => {
    const notices: unknown[] = []
    const off = setNotifier((n) => notices.push(n))
    await startImport(new QueryClient(), 'u1', 'registry.nasiko.dev/acme/bot:1.0.0', async () => ({
      agent_id: 'a1',
      build_id: null,
      container_name: 'c',
      status: 'success',
    }))
    off()
    expect(notices).toHaveLength(1)
    expect(importFor('u1')).toBeNull()
  })

  it('does not follow a GitHub clone that answers after sign-out cleared deploy work', async () => {
    let release!: () => void
    server.use(
      http.post('*/api/github/clone', async () => {
        await new Promise<void>((r) => {
          release = r
        })
        return HttpResponse.json(
          {
            success: true,
            message: 'queued',
            agent_name: 'fresh-agent',
            upload_id: '5eed000b-0000-4000-8000-000000000555',
          },
          { status: 202 },
        )
      }),
    )
    renderApp('/deploy?method=github')
    await pick('acme/fresh-agent')
    await userEvent.click(screen.getByRole('button', { name: 'Deploy acme/fresh-agent' }))
    await waitFor(() => expect(release).toBeTypeOf('function'))
    clearUploads()
    release()
    await new Promise((r) => setTimeout(r, 50))
    expect(followedCount()).toBe(0)
    expect(clonedRepo('5eed000b-0000-4000-8000-000000000555')).toBeNull()
  })

  it('says a branch is needed (in words, not only a red border) and focuses it', async () => {
    renderApp('/deploy?method=github')
    await pick('acme/fresh-agent')
    const branch = screen.getByRole('textbox', { name: copy.github.branch })
    await userEvent.clear(branch)
    await userEvent.click(screen.getByRole('button', { name: 'Deploy acme/fresh-agent' }))
    expect(await screen.findByText(copy.github.branchRequired)).toBeInTheDocument()
    expect(document.activeElement).toBe(branch)
    expect(branch).toHaveAccessibleDescription(copy.github.branchRequired)
  })
})
