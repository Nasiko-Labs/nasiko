/**
 * The config editor's key rules and edit safety, and custom providers (plans/feat-llm-router.md §4.3, §4.6, §8): the
 * secrets list re-read on Add a key and before submit, the replace warning, save-with-key failure copy, the missing
 * saved key 400, key hygiene in the caches, "Changed elsewhere" on a focus refetch, keyboard fallback reordering, and
 * custom providers as superuser and normal user.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks, routerMockState } from '@/mocks/handlers'
import { configId } from '@/mocks/router'
import { ADMIN_ID } from '@/mocks/seed-harness'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequestBodies, recordRequests, server } from '@/test/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, superuser: null, routerVariants: [] }))

const KEY = 'sk-test-0123456789abcdef'
const ready = async () => {
  await screen.findByRole('table', { name: copy.agentsTitle })
  await waitFor(() => expect(screen.queryAllByText(copy.readingRouting)).toHaveLength(0))
}
const announcer = () => screen.getByTestId('router-announcer')
/** The page shows one tab at a time (Agents first). */
const showTab = async (name: string) => {
  const tab = screen.getByRole('tab', { name })
  if (tab.getAttribute('aria-selected') !== 'true') await userEvent.click(tab)
}
/** Picks an option from a shadcn Select (it opens a listbox; there is no native select to target). */
const choose = async (combobox: HTMLElement, option: string) => {
  await userEvent.click(combobox)
  await userEvent.click(await screen.findByRole('option', { name: option }))
}
const openNew = async () => {
  await showTab(copy.anchors.configs)
  await userEvent.click(screen.getByRole('button', { name: copy.newConfig }))
  const sheet = await screen.findByRole('dialog', { name: copy.editorNew })
  await within(sheet).findByLabelText(copy.fieldName)
  return sheet
}
const openEdit = async (name: string) => {
  await showTab(copy.anchors.configs)
  await userEvent.click(screen.getByRole('button', { name: copy.configActions(name) }))
  await userEvent.click(await screen.findByRole('menuitem', { name: copy.edit }))
  const sheet = await screen.findByRole('dialog', { name: copy.editorEdit(name) })
  await within(sheet).findByLabelText(copy.fieldName)
  return sheet
}
const fillNew = async (sheet: HTMLElement, name: string, secretName: string) => {
  await userEvent.type(within(sheet).getByLabelText(copy.fieldName), name)
  await choose(within(sheet).getByLabelText(copy.fieldProvider), 'openai')
  await userEvent.type(within(sheet).getByLabelText(copy.fieldModel), 'gpt-4o')
  await userEvent.click(within(sheet).getByRole('radio', { name: copy.keyAdd }))
  const nameInput = within(sheet).getByLabelText(copy.secretName)
  await userEvent.clear(nameInput)
  await userEvent.type(nameInput, secretName)
  await userEvent.type(within(sheet).getByLabelText(copy.keyValue), KEY)
}

describe('keys', () => {
  it('re-reads /api/secrets when Add a key opens and before submit, and shows the containers notice', async () => {
    const calls = recordRequests()
    renderApp('/router')
    await ready()
    const sheet = await openNew()
    const before = calls.urls.filter((u) => u.pathname === '/api/secrets').length
    await userEvent.click(within(sheet).getByRole('radio', { name: copy.keyAdd }))
    await waitFor(() =>
      expect(calls.urls.filter((u) => u.pathname === '/api/secrets').length).toBe(before + 1),
    )
    expect(within(sheet).getByText(copy.keyContainers)).toBeInTheDocument()
    await fillNew(sheet, 'with-key', 'NEW_OPENAI_KEY')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await within(sheet).findByText(copy.notUsedYet)
    expect(calls.urls.filter((u) => u.pathname === '/api/secrets').length).toBeGreaterThanOrEqual(
      before + 2,
    )
    expect(calls.urls.filter((u) => /^\/api\/secrets\/.+/.test(u.pathname))).toEqual([])
    calls.stop()
  })

  it('an existing secret name asks to confirm the replace before sending', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    const sheet = await openNew()
    await fillNew(sheet, 'replacing', 'OPENAI_API_KEY')
    expect(within(sheet).getByText(copy.keyReplaces('OPENAI_API_KEY'))).toBeInTheDocument()
    // A key saved elsewhere after the page loaded still triggers the warning (the list is re-read at submit).
    await userEvent.clear(within(sheet).getByLabelText(copy.secretName))
    await userEvent.type(within(sheet).getByLabelText(copy.secretName), 'LATE_KEY')
    routerMockState()
      .secrets.get(ADMIN_ID)!
      .push({ id: 'late', name: 'LATE_KEY', created_at: '', updated_at: '' })
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    expect(await within(sheet).findByText(copy.keyReplaces('LATE_KEY'))).toBeInTheDocument()
    await rec.flush()
    expect(
      rec.requests.filter((r) => r.method === 'POST' && r.url.pathname === '/api/llm-configs'),
    ).toHaveLength(0)
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await within(sheet).findByText(copy.notUsedYet)
  })

  it('a failed save that sent a key says the key may have been saved', async () => {
    server.use(
      http.post('/api/llm-configs', () => new HttpResponse('internal error', { status: 500 })),
    )
    renderApp('/router')
    await ready()
    const sheet = await openNew()
    await fillNew(sheet, 'will-fail', 'FAIL_KEY')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    expect(await within(sheet).findByText(copy.keyMaybeSaved)).toBeInTheDocument()
    expect(sheet).not.toHaveTextContent(KEY)
  })

  it('a missing saved key is a 400 with a way out', async () => {
    server.use(
      http.post(
        '/api/llm-configs',
        () =>
          new HttpResponse("secret 'GONE_KEY' not found; provide secret_value to store it", {
            status: 400,
          }),
      ),
    )
    renderApp('/router')
    await ready()
    const sheet = await openNew()
    await userEvent.type(within(sheet).getByLabelText(copy.fieldName), 'missing')
    await userEvent.type(within(sheet).getByLabelText(copy.fieldModel), 'gpt-4o')
    await userEvent.click(within(sheet).getByRole('radio', { name: copy.keyUseSaved }))
    await choose(within(sheet).getByRole('combobox', { name: copy.keyUseSaved }), 'OPENAI_API_KEY')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    expect(await within(sheet).findByRole('alert')).toHaveTextContent(
      'That saved key no longer exists. Add the key again, or pick another saved key.',
    )
    expect(within(sheet).queryByText(copy.keyMaybeSaved)).toBeNull()
  })

  it('no query or mutation cache entry holds the key after a save', async () => {
    const { queryClient } = renderApp('/router')
    await ready()
    const sheet = await openNew()
    await fillNew(sheet, 'hygiene', 'HYGIENE_KEY')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await within(sheet).findByText(copy.notUsedYet)
    await userEvent.click(within(sheet).getByRole('button', { name: copy.done }))
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull())
    const dump = JSON.stringify([
      queryClient
        .getQueryCache()
        .getAll()
        .map((q) => q.state.data),
      queryClient
        .getMutationCache()
        .getAll()
        .map((m) => [m.state.variables, m.state.data]),
    ])
    expect(dump).not.toContain(KEY)
  })
})

describe('edit safety', () => {
  it('a focus refetch during an edit keeps the edit and says it changed elsewhere', async () => {
    const { queryClient } = renderApp('/router')
    await ready()
    const sheet = await openEdit('research-tiers')
    const name = within(sheet).getByLabelText(copy.fieldName)
    await userEvent.clear(name)
    await userEvent.type(name, 'my-edit')
    const c = routerMockState().configs.find((x) => x.id === configId(2))!
    c.updated_at = new Date(now() + 60_000).toISOString()
    c.temperature = 0.9
    // The list refetches on focus; the clock is frozen, so force the refetch that focus would start.
    await queryClient.refetchQueries({ queryKey: ['router', 'configs'] })
    expect(await within(sheet).findByText(copy.changedElsewhere)).toBeInTheDocument()
    expect(name).toHaveValue('my-edit')
  })

  it('fallbacks reorder by keyboard and each move is announced', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    const sheet = await openEdit('anthropic-default')
    const add = within(sheet).getByRole('combobox', { name: copy.addFallback })
    await userEvent.type(add, 'openai/gpt-4o')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.addFallback }))
    within(sheet)
      .getByRole('button', { name: copy.moveUp('openai/gpt-4o') })
      .focus()
    await userEvent.keyboard('{Enter}')
    await waitFor(() => expect(announcer()).toHaveTextContent(copy.movedTo('openai/gpt-4o', 1)))
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(sheet).not.toBeInTheDocument())
    await rec.flush()
    expect(rec.requests.find((r) => r.method === 'PATCH')!.body).toEqual({
      fallback_models: ['openai/gpt-4o', 'openai/gpt-4o-mini'],
    })
  })
})

describe('custom providers', () => {
  const providers = () =>
    screen.getByRole('heading', { name: copy.providersTitle }).closest('section')!

  it('a normal user sees them but has no actions', async () => {
    configureMocks({ superuser: false })
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.providers)
    expect(within(providers()).getAllByText('Custom').length).toBeGreaterThan(0)
    expect(within(providers()).queryByRole('button', { name: copy.addCustom })).toBeNull()
    expect(
      within(providers()).queryByRole('button', { name: copy.configActions('Custom') }),
    ).toBeNull()
  })

  it('add rejects non-http URLs, tests with the typed key and saves', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.providers)
    await userEvent.click(within(providers()).getByRole('button', { name: copy.addCustom }))
    const sheet = await screen.findByRole('dialog', { name: copy.cpNew })
    await userEvent.type(within(sheet).getByLabelText(copy.cpName), 'Local vLLM')
    await userEvent.type(within(sheet).getByLabelText(copy.cpBaseUrl), 'file:///etc/passwd')
    expect(within(sheet).getByText(copy.cpBaseUrlBad)).toBeInTheDocument()
    await userEvent.clear(within(sheet).getByLabelText(copy.cpBaseUrl))
    await userEvent.type(within(sheet).getByLabelText(copy.cpBaseUrl), 'http://localhost:8000/v1')
    expect(within(sheet).getByRole('button', { name: copy.cpTest })).toBeDisabled()
    await userEvent.type(within(sheet).getByLabelText(copy.cpKey), KEY)
    await userEvent.type(within(sheet).getByLabelText(copy.cpDefaultModel), 'custom-large')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.cpTest }))
    expect(await within(sheet).findByText(copy.cpTestOk(2))).toBeInTheDocument()
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(sheet).not.toBeInTheDocument())
    await rec.flush()
    expect(rec.requests.find((r) => r.url.pathname === '/api/custom-providers/test')!.body).toEqual(
      { base_url: 'http://localhost:8000/v1', api_key: KEY, kind: 'openai', model: 'custom-large' },
    )
    const create = rec.requests.find(
      (r) => r.method === 'POST' && r.url.pathname === '/api/custom-providers',
    )!.body
    expect(create).toMatchObject({
      display_name: 'Local vLLM',
      base_url: 'http://localhost:8000/v1',
      kind: 'openai',
    })
    expect(create).not.toHaveProperty('api_version')
    // The server's create drops default_model, so a PATCH sets it.
    expect(rec.requests.find((r) => r.method === 'PATCH')!.body).toEqual({
      default_model: 'custom-large',
    })
  })

  it('an Azure endpoint needs its API version, and the type is locked on edit', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.providers)
    await userEvent.click(within(providers()).getByRole('button', { name: copy.addCustom }))
    const sheet = await screen.findByRole('dialog', { name: copy.cpNew })
    expect(within(sheet).queryByLabelText(copy.cpApiVersion)).toBeNull()
    await userEvent.click(within(sheet).getByLabelText(copy.cpKind))
    await userEvent.click(await screen.findByRole('option', { name: copy.cpKinds['azure-openai'] }))
    await userEvent.type(within(sheet).getByLabelText(copy.cpName), 'Azure prod')
    await userEvent.type(
      within(sheet).getByLabelText(copy.cpBaseUrl),
      'https://prod.openai.azure.com/openai',
    )
    await userEvent.type(within(sheet).getByLabelText(copy.cpKey), KEY)
    expect(within(sheet).getByRole('button', { name: copy.save })).toBeDisabled()
    expect(within(sheet).getByRole('button', { name: copy.cpTest })).toBeDisabled()
    await userEvent.type(within(sheet).getByLabelText(copy.cpApiVersion), '2024-10-21')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(sheet).not.toBeInTheDocument())
    await rec.flush()
    expect(
      rec.requests.find((r) => r.method === 'POST' && r.url.pathname === '/api/custom-providers')!
        .body,
    ).toMatchObject({ kind: 'azure-openai', api_version: '2024-10-21' })

    await userEvent.click(
      within(providers()).getByRole('button', { name: copy.configActions('Azure prod') }),
    )
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.edit }))
    const edit = await screen.findByRole('dialog', { name: copy.cpEdit('Azure prod') })
    expect(within(edit).getByLabelText(copy.cpKind)).toBeDisabled()
    expect(within(edit).getByLabelText(copy.cpBaseUrl)).toHaveValue('https://prod.openai.azure.com')
    expect(within(edit).getByLabelText(copy.cpApiVersion)).toHaveValue('2024-10-21')
  })

  it('a failing test shows the provider’s error as text', async () => {
    configureMocks({ routerVariants: ['router-custom-down'] })
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.providers)
    await userEvent.click(within(providers()).getByRole('button', { name: copy.addCustom }))
    const sheet = await screen.findByRole('dialog', { name: copy.cpNew })
    await userEvent.type(within(sheet).getByLabelText(copy.cpBaseUrl), 'http://localhost:9/v1')
    await userEvent.type(within(sheet).getByLabelText(copy.cpKey), KEY)
    await userEvent.click(within(sheet).getByRole('button', { name: copy.cpTest }))
    expect(await within(sheet).findByText(/connection refused/)).toBeInTheDocument()
  })

  it('sync announces the model count', async () => {
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.providers)
    await userEvent.click(
      within(providers()).getByRole('button', { name: copy.configActions('Custom') }),
    )
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.syncNow }))
    await waitFor(() => expect(announcer()).toHaveTextContent(/^Synced Custom: \d+ models\.$/))
  })

  it('delete of a referenced provider lists the configs that use it', async () => {
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.providers)
    await userEvent.click(
      within(providers()).getByRole('button', { name: copy.configActions('Custom') }),
    )
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.delete }))
    const dialog = await screen.findByRole('alertdialog')
    await userEvent.click(within(dialog).getByRole('button', { name: copy.delete }))
    expect(await within(dialog).findByText(copy.cpDeleteRefs('local-models'))).toBeInTheDocument()
  })
})

describe('save gate', () => {
  it('an edit with nothing changed can’t be saved', async () => {
    renderApp('/router')
    await ready()
    const sheet = await openEdit('research-tiers')
    expect(within(sheet).getByRole('button', { name: copy.save })).toBeDisabled()
    await userEvent.type(within(sheet).getByLabelText(copy.fieldName), '-2')
    expect(within(sheet).getByRole('button', { name: copy.save })).toBeEnabled()
  })
})

describe('review fixes', () => {
  it('rotating a key under the same name sends the name with the value', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    const sheet = await openEdit('anthropic-default')
    await userEvent.click(within(sheet).getByRole('radio', { name: copy.keyAdd }))
    expect(within(sheet).getByLabelText(copy.secretName)).toHaveValue('ANTHROPIC_API_KEY')
    await userEvent.type(within(sheet).getByLabelText(copy.keyValue), KEY)
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    // The name exists, so the first Save asks to confirm the replace.
    await within(sheet).findByText(copy.keyReplaces('ANTHROPIC_API_KEY'))
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(sheet).not.toBeInTheDocument())
    await rec.flush()
    const patch = rec.requests.find(
      (r) => r.method === 'PATCH' && r.url.pathname === `/api/llm-configs/${configId(1)}`,
    )!
    expect(patch.body).toEqual({ api_key_secret_name: 'ANTHROPIC_API_KEY', secret_value: KEY })
  })

  it('double-clicking Save with a new key sends one POST', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    const sheet = await openNew()
    await fillNew(sheet, 'dbl', 'DBL_KEY')
    await userEvent.dblClick(within(sheet).getByRole('button', { name: copy.save }))
    await within(sheet).findByText(copy.notUsedYet)
    await rec.flush()
    expect(
      rec.requests.filter((r) => r.method === 'POST' && r.url.pathname === '/api/llm-configs'),
    ).toHaveLength(1)
  })

  it('a failed saved-keys read still opens the editor, with Retry and Add a key', async () => {
    configureMocks({ routerVariants: ['router-secrets-fail'] })
    renderApp('/router')
    await ready()
    const sheet = await openNew()
    expect(within(sheet).getByText(copy.secretsFailed)).toBeInTheDocument()
    expect(within(sheet).getByRole('radio', { name: copy.keyAdd })).toBeEnabled()
    expect(within(sheet).getByRole('radio', { name: copy.keyUseSaved })).toBeDisabled()
    expect(within(sheet).queryByText(copy.catalogFailed)).toBeNull()
    expect(within(sheet).getByRole('button', { name: copy.retry })).toBeInTheDocument()
  })

  it('no saved keys: Add a key is preselected and saved keys are off', async () => {
    configureMocks({ routerVariants: ['router-no-secrets'] })
    renderApp('/router')
    await ready()
    const sheet = await openNew()
    expect(within(sheet).getByRole('radio', { name: copy.keyAdd })).toBeChecked()
    expect(within(sheet).getByRole('radio', { name: copy.keyUseSaved })).toBeDisabled()
    expect(within(sheet).getByText(copy.noSavedKeys)).toBeInTheDocument()
  })

  it('a failed catalog keeps the editor from saving', async () => {
    configureMocks({ routerVariants: ['router-catalog-fail'] })
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    await userEvent.click(screen.getByRole('button', { name: copy.newConfig }))
    const sheet = await screen.findByRole('dialog', { name: copy.editorNew })
    await within(sheet).findByLabelText(copy.fieldName)
    await userEvent.type(within(sheet).getByLabelText(copy.fieldName), 'x')
    await userEvent.type(within(sheet).getByLabelText(copy.fieldModel), 'gpt-4o')
    expect(within(sheet).getByRole('button', { name: copy.save })).toBeDisabled()
  })

  it('emptying a set tier model is blocked, and Duplicate without this field opens a duplicate without it', async () => {
    renderApp('/router')
    await ready()
    const sheet = await openEdit('research-tiers')
    const tier = within(sheet).getByLabelText(copy.tierLabels[0])
    await userEvent.clear(tier)
    expect(within(sheet).getByText(copy.cantClear, { exact: false })).toBeInTheDocument()
    expect(within(sheet).getByRole('button', { name: copy.save })).toBeDisabled()
    await userEvent.click(within(sheet).getByRole('button', { name: copy.duplicateWithout }))
    const dup = await screen.findByRole('dialog', { name: copy.editorDuplicate('research-tiers') })
    await within(dup).findByLabelText(copy.fieldName)
    expect(within(dup).getByLabelText(copy.fieldName)).toHaveValue('research-tiers copy')
    expect(within(dup).getByLabelText(copy.tierLabels[0])).toHaveValue('')
    expect(within(dup).getByLabelText(copy.tierLabels[1])).toHaveValue('gpt-4o-mini')
  })

  it('a moved fallback keeps keyboard focus on its arrow', async () => {
    renderApp('/router')
    await ready()
    const sheet = await openEdit('anthropic-default')
    await userEvent.type(
      within(sheet).getByRole('combobox', { name: copy.addFallback }),
      'openai/gpt-4o',
    )
    await userEvent.click(within(sheet).getByRole('button', { name: copy.addFallback }))
    within(sheet)
      .getByRole('button', { name: copy.moveUp('openai/gpt-4o') })
      .focus()
    await userEvent.keyboard('{Enter}')
    // Now first: Move up is disabled, so focus lands on its Move down.
    expect(document.activeElement).toBe(
      within(sheet).getByRole('button', { name: copy.moveDown('openai/gpt-4o') }),
    )
    await userEvent.keyboard('{Enter}')
    expect(document.activeElement).toBe(
      within(sheet).getByRole('button', { name: copy.moveUp('openai/gpt-4o') }),
    )
  })

  it('an Edit opened from a row menu returns focus to that menu’s trigger', async () => {
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    const trigger = screen.getByRole('button', { name: copy.configActions('research-tiers') })
    const sheet = await openEdit('research-tiers')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.cancel }))
    await waitFor(() => expect(trigger).toHaveFocus())
  })

  it('a custom provider edit without a key keeps the saved one (no api_key sent)', async () => {
    const rec = recordRequestBodies()
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.providers)
    const providers = screen.getByRole('heading', { name: copy.providersTitle }).closest('section')!
    await userEvent.click(
      within(providers).getByRole('button', { name: copy.configActions('Custom') }),
    )
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.edit }))
    const sheet = await screen.findByRole('dialog', { name: copy.cpEdit('Custom') })
    expect(within(sheet).getByText(`${copy.keySaved}. ${copy.cpKeyKeep}`)).toBeInTheDocument()
    await userEvent.clear(within(sheet).getByLabelText(copy.cpName))
    await userEvent.type(within(sheet).getByLabelText(copy.cpName), 'Local')
    await userEvent.click(within(sheet).getByRole('button', { name: copy.save }))
    await waitFor(() => expect(sheet).not.toBeInTheDocument())
    await rec.flush()
    const patch = rec.requests.find(
      (r) => r.method === 'PATCH' && r.url.pathname.startsWith('/api/custom-providers/'),
    )!
    expect(patch.body).toMatchObject({ display_name: 'Local' })
    expect(patch.body).not.toHaveProperty('api_key')
  })
})

describe('custom provider host change (review D1)', () => {
  it('a new host needs the key again; the same host keeps the saved one', async () => {
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.providers)
    const providers = screen.getByRole('heading', { name: copy.providersTitle }).closest('section')!
    await userEvent.click(
      within(providers).getByRole('button', { name: copy.configActions('Custom') }),
    )
    await userEvent.click(await screen.findByRole('menuitem', { name: copy.edit }))
    const sheet = await screen.findByRole('dialog', { name: copy.cpEdit('Custom') })
    const url = within(sheet).getByLabelText(copy.cpBaseUrl)
    await userEvent.clear(url)
    await userEvent.type(url, 'https://evil.example.com/v1')
    expect(within(sheet).getByText(copy.cpKeyNewHost)).toBeInTheDocument()
    expect(within(sheet).getByRole('button', { name: copy.save })).toBeDisabled()
    await userEvent.type(within(sheet).getByLabelText(copy.cpKey), KEY)
    expect(within(sheet).getByRole('button', { name: copy.save })).toBeEnabled()
    await userEvent.clear(within(sheet).getByLabelText(copy.cpKey))
    // Same origin, new path: the saved key may stay.
    await userEvent.clear(url)
    await userEvent.type(url, 'http://localhost:11434/v2')
    expect(within(sheet).queryByText(copy.cpKeyNewHost)).toBeNull()
    expect(within(sheet).getByRole('button', { name: copy.save })).toBeEnabled()
  })
})

// Regression: ISSUE-001 (/qa 2026-09-28, .gstack/qa-reports/qa-report-localhost-2026-09-28.md): untouched fields
// showed errors.
describe('field errors', () => {
  it('a new config opens without error borders; leaving a required field empty marks it', async () => {
    renderApp('/router')
    await ready()
    await showTab(copy.anchors.configs)
    await userEvent.click(screen.getByRole('button', { name: copy.newConfig }))
    const sheet = await screen.findByRole('dialog', { name: copy.editorNew })
    const name = await within(sheet).findByLabelText(copy.fieldName)
    expect(sheet.querySelectorAll('[aria-invalid="true"]')).toHaveLength(0)
    expect(within(sheet).queryByText(copy.timing, { selector: 'p[id]' })).toBeNull()
    await userEvent.click(name)
    await userEvent.tab()
    expect(name).toHaveAttribute('aria-invalid', 'true')
  })
})
