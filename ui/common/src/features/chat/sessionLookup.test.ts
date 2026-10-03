/**
 * The chat lookup cache (sessionLookup.ts): deleting a chat forgets its row
 * (useDeleteChat → forgetSessionLookup), so a deleted chat found past the rail can't come back
 * from the cache.
 */
import { http, HttpResponse } from 'msw'
import { afterEach, describe, expect, it } from 'vitest'
import { server } from '@/test/setup'
import { clearSessionLookup, forgetSessionLookup, lookupSession } from './sessionLookup'
import type { ChatSessionRow } from './types'

const SID = '5eedc000-0000-4000-8000-00000000e001'
const OTHER = '5eedc000-0000-4000-8000-00000000e002'
const row = (session_id: string) =>
  ({
    session_id,
    user_id: 'u',
    agent_id: null,
    agent_url: '/api/orchestrator/a2a',
    title: 'T',
    created_at: '',
    updated_at: '',
  }) as ChatSessionRow

afterEach(() => clearSessionLookup())

describe('forgetSessionLookup', () => {
  it('after a found row is forgotten, the next lookup asks the server again', async () => {
    let gets = 0
    let rows = [row(SID), row(OTHER)]
    server.use(
      http.get('/api/chat/sessions', () => {
        gets++
        return HttpResponse.json({
          data: rows,
          has_more: false,
          next_cursor: null,
          prev_cursor: null,
        })
      }),
    )
    expect(await lookupSession(SID)).toMatchObject({ status: 'found', row: { session_id: SID } })
    expect(await lookupSession(OTHER)).toMatchObject({ status: 'found' })
    expect(gets).toBe(2)
    // Cached: no request.
    expect(await lookupSession(SID)).toMatchObject({ status: 'found' })
    expect(gets).toBe(2)

    // The chat is deleted on the server and forgotten here.
    rows = [row(OTHER)]
    forgetSessionLookup(SID)
    expect(await lookupSession(SID)).toEqual({ status: 'absent' })
    expect(gets).toBe(3)
    // Only that chat's row was dropped.
    expect(await lookupSession(OTHER)).toMatchObject({ status: 'found' })
    expect(gets).toBe(3)
  })
})
