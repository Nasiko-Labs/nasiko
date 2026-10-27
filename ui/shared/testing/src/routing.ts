/**
 * An agent routing answer for page tests (`GET /api/agents/{id}/llm-config`): every agent on a `provider` config,
 * pinned to `pinned`. The router mock answers its own seed's (Anthropic) configs; budget lever tests need the
 * provider their traffic came from.
 */
import { http, HttpResponse } from 'msw'

export function routingAnswer(provider: string, pinned: string | null) {
  return http.get('/api/agents/:id/llm-config', ({ params }) =>
    HttpResponse.json({
      data: {
        agent_id: params.id,
        llm_config_id: null,
        llm_config: {
          id: 'c1',
          name: provider,
          provider,
          model: null,
          fallback_models: [],
          temperature: null,
          max_tokens: null,
          api_key_secret_name: null,
          pinned: false,
          pinned_model: null,
          tier1_model: null,
          tier2_model: null,
          tier3_model: null,
          is_default: true,
        },
        source: 'owner-default',
        inbound_format: 'openai',
        pinned_model: pinned,
      },
      status_code: 200,
      message: 'ok',
    }),
  )
}
