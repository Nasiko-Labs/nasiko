/** The attachment rules (§7.7): what the composer accepts, and what it turns away. */
import { describe, expect, it } from 'vitest'
import { addFiles, asFileParts, fileUrl, mediaKind, removeFile } from './attachments'
import { dispatchBody } from './send'
import { tuning } from './tuning'

const file = (name: string, size: number, type = 'text/plain') =>
  new File([new Uint8Array(size)], name, { type })

let n = 0
const newId = () => `id-${++n}`

describe('mediaKind', () => {
  it('names each kind from its mime, and an unknown one is a plain file', () => {
    expect(mediaKind('image/png')).toBe('image')
    expect(mediaKind('audio/mpeg')).toBe('audio')
    expect(mediaKind('video/mp4')).toBe('video')
    expect(mediaKind('application/pdf')).toBe('pdf')
    expect(mediaKind('application/octet-stream')).toBe('file')
    // A row saved before the server guessed a type must not render as a broken <img>.
    expect(mediaKind(undefined)).toBe('file')
    expect(mediaKind('')).toBe('file')
  })
})

describe('what may be attached', () => {
  it('takes every kind, not just the ones it can render: nothing is filtered by type', () => {
    const chosen = [
      file('photo.png', 10, 'image/png'),
      file('note.m4a', 10, 'audio/mp4'),
      file('clip.mov', 10, 'video/quicktime'),
      file('report.pdf', 10, 'application/pdf'),
      file('data.csv', 10, 'text/csv'),
      file('archive.zip', 10, 'application/zip'),
      // A type the browser didn't recognise: still attachable, shown as a plain file.
      file('mystery.dat', 10, ''),
    ]
    const { files, rejected } = addFiles([], chosen, newId)
    expect(files.map((f) => f.file.name)).toEqual(chosen.map((f) => f.name))
    expect(rejected).toEqual([])
    expect(files.map((f) => mediaKind(f.file.type))).toEqual([
      'image',
      'audio',
      'video',
      'pdf',
      'file',
      'file',
      'file',
    ])
  })
})

describe('addFiles', () => {
  it('keeps the order chosen and accumulates across calls', () => {
    const first = addFiles([], [file('a.txt', 10), file('b.txt', 10)], newId)
    const second = addFiles(first.files, [file('c.txt', 10)], newId)
    expect(second.files.map((f) => f.file.name)).toEqual(['a.txt', 'b.txt', 'c.txt'])
    expect(second.rejected).toEqual([])
  })

  it('attaches the acceptable half of a batch and says what it dropped', () => {
    const { files, rejected } = addFiles(
      [],
      [file('ok.txt', 10), file('huge.bin', tuning.ATTACHMENT_MAX_BYTES + 1)],
      newId,
    )
    expect(files.map((f) => f.file.name)).toEqual(['ok.txt'])
    expect(rejected).toEqual([{ name: 'huge.bin', reason: 'too-large' }])
  })

  it('refuses past the count limit, counting what is already attached', () => {
    const full = addFiles(
      [],
      Array.from({ length: tuning.MAX_ATTACHMENTS }, (_, i) => file(`f${i}.txt`, 1)),
      newId,
    )
    const { files, rejected } = addFiles(full.files, [file('extra.txt', 1)], newId)
    expect(files).toHaveLength(tuning.MAX_ATTACHMENTS)
    expect(rejected).toEqual([{ name: 'extra.txt', reason: 'too-many' }])
  })

  it('refuses a file that would take the message past the total budget', () => {
    const half = Math.floor(tuning.ATTACHMENTS_MAX_TOTAL_BYTES / 2)
    const first = addFiles([], [file('a.bin', half)], newId)
    const { files, rejected } = addFiles(first.files, [file('b.bin', half + 10)], newId)
    expect(files.map((f) => f.file.name)).toEqual(['a.bin'])
    expect(rejected).toEqual([{ name: 'b.bin', reason: 'over-budget' }])
  })

  it('refuses an empty entry: that is how a dragged folder arrives', () => {
    const { files, rejected } = addFiles([], [file('a-folder', 0)], newId)
    expect(files).toEqual([])
    expect(rejected).toEqual([{ name: 'a-folder', reason: 'empty' }])
  })
})

describe('removeFile', () => {
  it('drops only the named one', () => {
    const { files } = addFiles([], [file('a.txt', 1), file('b.txt', 1)], newId)
    expect(removeFile(files, files[0].id).map((f) => f.file.name)).toEqual(['b.txt'])
  })
})

describe('fileUrl', () => {
  it('encodes the id: it is the only part of the path a caller supplies', () => {
    expect(fileUrl('a/b?c')).toBe('/api/chat/files/a%2Fb%3Fc/download')
  })
})

describe('asFileParts', () => {
  it('describes the files a turn is sending, before any upload has happened', () => {
    expect(asFileParts([file('shot.png', 7, 'image/png')])).toEqual([
      { id: 'pending-0', name: 'shot.png', size: 7, mime: 'image/png' },
    ])
  })
})

describe('dispatchBody', () => {
  const base = { rpcId: 'r', messageId: 'm', sessionId: 's', text: 'hi' } as const

  it('names the attachments in metadata, beside the target', () => {
    const body = dispatchBody({ ...base, chatMode: 'direct', agentId: 'a', fileIds: ['f1', 'f2'] })
    expect(body.params.metadata).toEqual({
      agent_id: 'a',
      session_id: 's',
      file_ids: ['f1', 'f2'],
    })
  })

  it('sends no file_ids key at all without attachments, on either path', () => {
    const direct = dispatchBody({ ...base, chatMode: 'direct', agentId: 'a' })
    const routed = dispatchBody({ ...base, chatMode: 'routed', fileIds: [] })
    expect(direct.params.metadata).toEqual({ agent_id: 'a', session_id: 's' })
    expect(routed.params.metadata).toEqual({ session_id: 's' })
  })
})
