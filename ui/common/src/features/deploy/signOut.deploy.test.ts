/**
 * Eng review R3 / task E3: sign-out aborts this tab's uploads and drops followed builds, after chat and drafts (the R6
 * order), so nothing of the previous account finishes or toasts for the next one. Another account's sign-in in another
 * tab reloads this one (sessionSync.ts), which ends uploads with the page, so there is no in-place path to test.
 */
import { QueryClient } from '@tanstack/react-query'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { clearLocalState } from '@/app/shell/signOut'
import { follow, followedCount, clearFollower } from './follower'
import { clearUploads, setXhrFactory, startUpload, uploadFor, type XhrFactory } from './uploads'

class HeldXhr {
  static last: HeldXhr | null = null
  aborted = false
  upload = { onprogress: null as unknown }
  withCredentials = false
  onload: (() => void) | null = null
  onerror: (() => void) | null = null
  onabort: (() => void) | null = null
  constructor() {
    HeldXhr.last = this
  }
  open() {}
  setRequestHeader() {}
  send() {}
  abort() {
    this.aborted = true
    this.onabort?.()
  }
}

let prev: XhrFactory | null = null
afterEach(() => {
  if (prev) setXhrFactory(prev)
  clearUploads()
  clearFollower()
})

function startWork(qc: QueryClient) {
  prev = setXhrFactory(() => new HeldXhr() as unknown as XMLHttpRequest)
  void startUpload(qc, 'u1', new Blob(['z']), 'a.zip', { name: 'a' })
  follow(qc, '5eed000b-0000-4000-8000-000000000777', 'b')
  expect(uploadFor('u1')?.phase).toBe('uploading')
  expect(followedCount()).toBe(1)
}

describe('deploy work on sign-out (R3)', () => {
  it('sign-out aborts the upload and drops followed builds', async () => {
    const qc = new QueryClient()
    startWork(qc)
    await clearLocalState(qc, 'u1')
    expect(HeldXhr.last!.aborted).toBe(true)
    expect(uploadFor('u1')).toBeNull()
    expect(followedCount()).toBe(0)
  })

  it('clears after chat and drafts (R6 order)', async () => {
    const qc = new QueryClient()
    startWork(qc)
    const order: string[] = []
    const drafts = await import('@/features/chat/drafts')
    const spy = vi.spyOn(drafts, 'clearDrafts').mockImplementation(() => {
      order.push(`drafts, upload ${uploadFor('u1')?.phase ?? 'gone'}`)
    })
    await clearLocalState(qc, 'u1')
    spy.mockRestore()
    expect(order).toEqual(['drafts, upload uploading'])
    expect(uploadFor('u1')).toBeNull()
  })
})
