/**
 * Request classification ([classifier] companion): the status card against each mock deployment, who may
 * preview, the submit flow and its result cards, fallback and abstention rendering, errors with retry,
 * stale-result protection when two previews overlap, secret absence, keyboard use and axe.
 */
import { screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import axe from 'axe-core'
import { delay, http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { configureMocks } from '@/mocks/handlers'
import { now, seed, setupPinnedSeed } from '@/test/pinnedSeed'
import { renderApp } from '@/test/renderApp'
import { recordRequestBodies, server } from '@/test/setup'
import { copy } from './copy'

setupPinnedSeed()
afterEach(() => configureMocks({ seed, now, loggedIn: true, superuser: null, routerVariants: [] }))

const open = async () => {
  renderApp('/router?tab=classification')
  await screen.findByRole('heading', { level: 2, name: copy.classifierTitle })
  await waitFor(() => expect(screen.queryByText(copy.classifierLoading)).toBeNull())
  await screen.findByRole('heading', { level: 3, name: copy.classifierStatusTitle })
}
const query = () => screen.getByLabelText(copy.classifierQuery)
const testButton = () => screen.getByRole('button', { name: copy.classifierTest })
const results = () => screen.findByRole('region', { name: copy.classifierResultsLabel })
const card = (name: string) => within(screen.getByRole('article', { name }))
/** Bodies of the preview POSTs sent so far. */
const previews = () => {
  const rec = recordRequestBodies()
  return async () => {
    await rec.flush()
    return rec.requests
      .filter((r) => r.method === 'POST' && r.url.pathname === '/api/llm-router/classifier/preview')
      .map((r) => r.body)
  }
}
const SETUP = /Set CLASSIFIER_BACKEND=jev and TYPESAFE_API_KEY on the server/

describe('status', () => {
  it('shows the configured Jev deployment, its model, floor and counters, without any key', async () => {
    await open()
    expect(screen.getByText(copy.classifierJevExperimental)).toBeInTheDocument()
    expect(screen.getByText(copy.classifierConfigured('Jev'))).toBeInTheDocument()
    expect(screen.getByText(copy.classifierEffective('Jev'))).toBeInTheDocument()
    expect(screen.getByText(copy.classifierModel('jev-1.13.0'))).toBeInTheDocument()
    expect(screen.getByText(copy.classifierEndpoint('api.typesafe.ai'))).toBeInTheDocument()
    expect(screen.getByText(copy.classifierFloor(0.35))).toBeInTheDocument()
    expect(screen.getByText(/128 classified since start/)).toBeInTheDocument()
    expect(document.body.textContent).not.toMatch(/sk-|TYPESAFE_API_KEY=/)
    // A working hosted backend shows no setup guidance, and names the backend not configured here.
    expect(screen.queryByText(copy.classifierSetupTitle + ':')).toBeNull()
    expect(screen.getByText(copy.classifierOtherBackends('Laya'))).toBeInTheDocument()
  })

  it('on the regex default, says so and shows the setup path to Jev', async () => {
    configureMocks({ routerVariants: ['router-classifier-regex'] })
    await open()
    expect(screen.getByText(copy.classifierRegexDefault)).toBeInTheDocument()
    expect(screen.getByText(copy.classifierEffective('Regex'))).toBeInTheDocument()
    expect(screen.getByText(SETUP)).toBeInTheDocument()
    expect(screen.getByText(/laya-setup\.sh/)).toBeInTheDocument()
    expect(screen.getByText(copy.classifierOtherBackends('Jev, Laya'))).toBeInTheDocument()
    // No backend picker when there is nothing to pick between.
    expect(screen.queryByLabelText(copy.classifierBackendPick)).toBeNull()
  })

  it('distinguishes a configured backend that cannot start from the one that answers', async () => {
    configureMocks({ routerVariants: ['router-classifier-unconfigured'] })
    await open()
    expect(screen.getByText(copy.classifierConfigured('Jev'))).toBeInTheDocument()
    expect(screen.getByText(copy.classifierEffective('Regex'))).toBeInTheDocument()
    expect(screen.getByTestId('classifier-init-error')).toHaveTextContent(/TYPESAFE_API_KEY/)
    expect(screen.getByText(SETUP)).toBeInTheDocument()
  })

  it('shows a loaded local Laya model with its files and no API fee, and names the unrun backend', async () => {
    configureMocks({ routerVariants: ['router-classifier-laya'] })
    await open()
    expect(screen.getByText(copy.classifierLayaExperimental)).toBeInTheDocument()
    expect(screen.getByText(copy.classifierLayaReady)).toBeInTheDocument()
    expect(screen.getByText(copy.classifierEffective('Laya'))).toBeInTheDocument()
    expect(screen.getByText(/Model files \/srv\/nasiko\/\.laya\/model/)).toBeInTheDocument()
    expect(screen.queryByText(/Endpoint /)).toBeNull()
    expect(screen.getByText(copy.classifierOtherBackends('Jev'))).toBeInTheDocument()
    await userEvent.type(query(), 'Design a rate limiter for a multi-tenant API')
    await userEvent.click(testButton())
    await results()
    const local = card(copy.classifierResultConfigured('Laya'))
    expect(local.getByText(copy.classifierAnsweredBy('Laya'))).toBeInTheDocument()
    expect(local.getByText(/local inference, no API fee/)).toBeInTheDocument()
    expect(local.getByText(/receptron\/laya-onnx@68f27df/)).toBeInTheDocument()
  })

  it('a Laya deployment whose bundle is missing says so, shows the setup script, and falls back', async () => {
    configureMocks({ routerVariants: ['router-classifier-laya-missing'] })
    await open()
    expect(screen.getByText(copy.classifierEffective('Regex'))).toBeInTheDocument()
    expect(screen.getByTestId('classifier-init-error')).toHaveTextContent(/laya\.onnx.*missing/)
    expect(screen.getByText(/laya-setup\.sh, then set CLASSIFIER_BACKEND=laya/)).toBeInTheDocument()
    expect(screen.queryByText(copy.classifierLayaReady)).toBeNull()
    await userEvent.type(query(), 'what is the capital of France?')
    await userEvent.click(testButton())
    await results()
    const local = card(copy.classifierResultConfigured('Laya'))
    expect(local.getByText(copy.classifierAnsweredBy('Regex'))).toBeInTheDocument()
    expect(local.getByText(/Fell back to regex: backend not configured/)).toBeInTheDocument()
  })

  it('shows a section error with Retry when the status read fails', async () => {
    server.use(
      http.get('/api/llm-router/classifier', () => HttpResponse.text('boom', { status: 500 })),
    )
    renderApp('/router?tab=classification')
    await screen.findByRole('heading', { level: 2, name: copy.classifierTitle })
    const retry = await screen.findByRole('button', { name: copy.retry })
    server.resetHandlers()
    await userEvent.click(retry)
    await screen.findByRole('heading', { level: 3, name: copy.classifierStatusTitle })
  })
})

describe('authorization', () => {
  it('a non-superuser sees the live status but cannot run a preview', async () => {
    configureMocks({ superuser: false })
    await open()
    expect(screen.getByText(copy.classifierNoRights)).toBeInTheDocument()
    expect(query()).toBeDisabled()
    expect(testButton()).toBeDisabled()
  })
})

describe('preview', () => {
  it('runs only on the button, sends query, context and backend, and shows both cards', async () => {
    const bodies = previews()
    await open()
    await userEvent.type(query(), 'Design a rate limiter for a multi-tenant API')
    await userEvent.type(screen.getByLabelText(copy.classifierContext), 'user: we have redis')
    expect(await bodies()).toHaveLength(0)
    await userEvent.click(testButton())
    const region = await results()
    expect(await bodies()).toHaveLength(1)
    expect((await bodies())[0]).toEqual({
      query: 'Design a rate limiter for a multi-tenant API',
      context: 'user: we have redis',
      backend: 'configured',
    })
    const hosted = card(copy.classifierResultConfigured('Jev'))
    expect(hosted.getByText(copy.classifierAnsweredBy('Jev'))).toBeInTheDocument()
    expect(hosted.getByText('Technical design')).toBeInTheDocument()
    expect(hosted.getByText(/of 5/)).toBeInTheDocument()
    expect(hosted.getByText(/probability the model gave this type/)).toBeInTheDocument()
    expect(hosted.getByText(/ms, including any fallback/)).toBeInTheDocument()
    expect(hosted.getByText(/input tokens · price not configured/)).toBeInTheDocument()
    expect(hosted.getByText(copy.classifierModelVersion('jev-1.13.0'))).toBeInTheDocument()
    const baseline = card(copy.classifierResultBaseline)
    expect(baseline.getByText(copy.classifierAnsweredBy('Regex'))).toBeInTheDocument()
    expect(baseline.getByText(/fixed placeholder, not calibrated/)).toBeInTheDocument()
    expect(baseline.getByText(copy.classifierCostNone)).toBeInTheDocument()
    expect(within(region).getByText(copy.classifierResultSame)).toBeInTheDocument()
    expect(screen.getByText(copy.classifierPreviewOnly)).toBeInTheDocument()
    // The polite live region announced the verdict.
    expect(screen.getByRole('status')).toHaveTextContent(/Classified as Technical design/)
  })

  it('an empty query is refused locally and never sent', async () => {
    const bodies = previews()
    await open()
    await userEvent.click(testButton())
    expect(screen.getByText(copy.classifierQueryRequired)).toBeInTheDocument()
    expect(query()).toHaveAttribute('aria-invalid', 'true')
    expect(await bodies()).toHaveLength(0)
    await userEvent.type(query(), 'x')
    expect(screen.queryByText(copy.classifierQueryRequired)).toBeNull()
  })

  it('the regex-only choice shows one card and sends backend=regex', async () => {
    const bodies = previews()
    await open()
    await userEvent.click(screen.getByRole('combobox', { name: copy.classifierBackendPick }))
    await userEvent.click(await screen.findByRole('option', { name: copy.classifierPickRegex }))
    await userEvent.type(query(), 'what is the capital of France?')
    await userEvent.click(testButton())
    await results()
    expect((await bodies())[0]).toMatchObject({ backend: 'regex' })
    expect(screen.getAllByRole('article')).toHaveLength(1)
    expect(card(copy.classifierResultBaseline).getByText('Factual lookup')).toBeInTheDocument()
  })

  it('an unconfigured hosted backend renders a regex fallback, never a Jev success', async () => {
    configureMocks({ routerVariants: ['router-classifier-unconfigured'] })
    await open()
    await userEvent.type(query(), 'what is the capital of France?')
    await userEvent.click(testButton())
    await results()
    const hosted = card(copy.classifierResultConfigured('Jev'))
    expect(hosted.getByText(copy.classifierAnsweredBy('Regex'))).toBeInTheDocument()
    expect(hosted.getByText(/Fell back to regex: backend not configured/)).toBeInTheDocument()
    expect(hosted.queryByText(/probability the model gave/)).toBeNull()
    expect(hosted.getByText(copy.classifierCostNone)).toBeInTheDocument()
  })

  it('a low-confidence hosted answer is marked as an abstention', async () => {
    server.use(
      http.post('/api/llm-router/classifier/preview', () =>
        HttpResponse.json({
          data: {
            backend: 'configured',
            configured_backend: 'jev',
            init_error: null,
            result: {
              answered_by: 'jev',
              disposition: 'abstained',
              request_type: 'general',
              complexity: 2,
              confidence: 0.31,
              latency_us: 240_000,
              input_truncated: true,
              diagnostics: {
                model_version: 'jev-1.13.0',
                type_probabilities: [],
                complexity_probabilities: [],
                complexity_expected: null,
                vendor_type_confidence: null,
                vendor_complexity_confidence: null,
                input_tokens: 600,
                output_tokens: 30,
                attempts: 1,
              },
            },
            baseline: {
              answered_by: 'regex',
              disposition: 'regex',
              request_type: 'general',
              complexity: 3,
              confidence: 0.3,
              latency_us: 12,
              input_truncated: false,
              diagnostics: null,
            },
          },
          status_code: 200,
          message: 'ok',
        }),
      ),
    )
    await open()
    await userEvent.type(query(), 'hmm')
    await userEvent.click(testButton())
    await results()
    const hosted = card(copy.classifierResultConfigured('Jev'))
    expect(hosted.getByText(copy.classifierAbstained)).toBeInTheDocument()
    expect(hosted.getByText(copy.classifierConfidenceHosted(31))).toBeInTheDocument()
    expect(hosted.getByText(copy.classifierTruncated)).toBeInTheDocument()
  })

  it('a timeout shows the mapped error with Retry, and Retry re-sends the same input', async () => {
    configureMocks({ routerVariants: ['router-classifier-fail'] })
    const bodies = previews()
    await open()
    await userEvent.type(query(), 'explain what this function does')
    await userEvent.click(testButton())
    await screen.findByText(copy.classifierTimedOut)
    expect(screen.getByText(copy.classifierTimedOutFix)).toBeInTheDocument()
    configureMocks({ routerVariants: [] })
    await userEvent.click(screen.getByRole('button', { name: copy.retry }))
    await results()
    const sent = await bodies()
    expect(sent).toHaveLength(2)
    expect(sent[1]).toEqual(sent[0])
    expect(screen.queryByText(copy.classifierTimedOut)).toBeNull()
  })

  it('a rate limit shows the mapped error', async () => {
    server.use(
      http.post('/api/llm-router/classifier/preview', () =>
        HttpResponse.text('rate limit exceeded, try again shortly', { status: 429 }),
      ),
    )
    await open()
    await userEvent.type(query(), 'hello')
    await userEvent.click(testButton())
    await screen.findByText(copy.classifierRateLimited)
  })

  it('a slower earlier preview never overwrites a newer one', async () => {
    let calls = 0
    server.use(
      http.post('/api/llm-router/classifier/preview', async () => {
        calls += 1
        const mine = calls
        // The first call answers last.
        await delay(mine === 1 ? 300 : 10)
        return HttpResponse.json({
          data: {
            backend: 'regex',
            configured_backend: 'regex',
            init_error: null,
            result: {
              answered_by: 'regex',
              disposition: 'regex',
              request_type: mine === 1 ? 'writing' : 'code_generation',
              complexity: 3,
              confidence: 0.5,
              latency_us: 10,
              input_truncated: false,
              diagnostics: null,
            },
            baseline: {
              answered_by: 'regex',
              disposition: 'regex',
              request_type: mine === 1 ? 'writing' : 'code_generation',
              complexity: 3,
              confidence: 0.5,
              latency_us: 10,
              input_truncated: false,
              diagnostics: null,
            },
          },
          status_code: 200,
          message: 'ok',
        })
      }),
    )
    configureMocks({ routerVariants: ['router-classifier-regex'] })
    await open()
    await userEvent.type(query(), 'first')
    await userEvent.click(testButton())
    // The button is busy; submit again through the form as soon as it is free.
    await waitFor(() => expect(testButton()).toBeEnabled(), { timeout: 2000 }).catch(() => {})
    await userEvent.clear(query())
    await userEvent.type(query(), 'second{Enter}')
    await userEvent.click(testButton()).catch(() => {})
    await waitFor(() => expect(calls).toBeGreaterThanOrEqual(2))
    await new Promise((r) => setTimeout(r, 400))
    expect(card(copy.classifierResultBaseline).getByText('Code generation')).toBeInTheDocument()
    expect(screen.queryByText('Writing')).toBeNull()
  })
})

describe('accessibility', () => {
  it('is keyboard operable and has no axe violations with results shown', async () => {
    await open()
    query().focus()
    await userEvent.keyboard('Explain why this closure needs move')
    await userEvent.tab() // context
    await userEvent.tab() // backend picker
    await userEvent.tab() // button
    expect(testButton()).toHaveFocus()
    await userEvent.keyboard('{Enter}')
    await results()
    const { violations } = await axe.run(document.body, {
      rules: { 'color-contrast': { enabled: false }, region: { enabled: false } },
    })
    expect(violations).toEqual([])
  })
})
