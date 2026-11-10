import { afterEach, describe, expect, it } from 'vitest'
import { resetChatMock } from './chatStore'

const url = (p: string) => new URL(p, globalThis.location.origin)
const sid = '5eedc000-0000-4000-8000-0000000000a1'

async function upload(name: string, type: string, bytes: number) {
  const boundary = 'nasiko-upload-boundary'
  const enc = new TextEncoder()
  const head = enc.encode(
    `--${boundary}\r\nContent-Disposition: form-data; name="file"; filename="${name}"\r\nContent-Type: ${type}\r\n\r\n`,
  )
  const tail = enc.encode(`\r\n--${boundary}--\r\n`)
  const body = new Uint8Array(head.length + bytes + tail.length)
  body.set(head, 0)
  body.set(tail, head.length + bytes)
  return fetch(url(`/api/chat/sessions/${sid}/files`), {
    method: 'POST',
    body,
    headers: { 'Content-Type': `multipart/form-data; boundary=${boundary}` },
  })
}

describe('chat upload limits', () => {
  afterEach(() => resetChatMock())

  it('refuses each file over its own type limit with the server wording', async () => {
    const created = await fetch(url('/api/chat/sessions'), {
      method: 'POST',
      body: JSON.stringify({ session_id: sid, first_prompt: 'files' }),
      headers: { 'Content-Type': 'application/json' },
    })
    expect(created.status).toBe(201)

    const zip = await upload('bundle.zip', 'application/zip', 11 * 1024 * 1024)
    expect(zip.status).toBe(413)
    expect(await zip.text()).toBe("file 'bundle.zip' is over the 10 MB limit for other files")

    const notes = await upload('notes.txt', 'text/plain', 1024 * 1024 + 1)
    expect(notes.status).toBe(413)
    expect(await notes.text()).toBe("file 'notes.txt' is over the 1 MB limit for text files")

    const image = await upload('fact.png', 'image/png', 3 * 1024 * 1024)
    expect(image.status).toBe(201)
  })
})
