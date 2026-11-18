/**
 * What a message may carry besides text (plan §7.7; D3's deferred attachments). Pure: the rules
 * for accepting a file, what kind of media it is, and where its bytes are read from. The IO is
 * `api.ts::uploadChatFiles` and the registry's upload step (`turnSend.ts`).
 *
 * Limits mirror the server's, so a file the composer accepts is one the turn can actually send:
 * count and per-file size are `chat/routes.rs` (`MAX_FILES_PER_UPLOAD`, `MAX_FILE_BYTES`), and the
 * per-message total is `a2a_dispatch.rs`'s `MAX_ATTACHED_BYTES` — the budget for base64'ing the
 * lot into one A2A message. Checking here means the user hears "too large" while choosing, not
 * after a 50 MB upload the dispatch then refuses.
 */
import { tuning } from './tuning'
import type { FilePart } from './types'

/** A file chosen in the composer, before the turn that sends it has uploaded it. */
export interface PendingFile {
  /** Local, for the chip's key and its remove button. Never the server's file id. */
  id: string
  file: File
  /** An object URL, for the kinds whose chip shows the thing itself. Revoked on remove (NE-A1). */
  preview?: string
}

export type MediaKind = 'image' | 'audio' | 'video' | 'pdf' | 'file'

/** Why a file was turned away. The composer turns each into one line of copy. */
export type RejectReason = 'too-many' | 'too-large' | 'over-budget' | 'empty'

export type SizeClass = keyof typeof tuning.ATTACHMENT_LIMITS

export interface Rejected {
  name: string
  reason: RejectReason
  kind?: SizeClass
}

const TEXT_TYPES = new Set([
  'application/json',
  'application/xml',
  'application/x-yaml',
  'application/yaml',
])
const TEXT_EXTENSIONS = new Set(['txt', 'md', 'csv', 'json', 'xml', 'yaml', 'yml', 'html', 'htm'])
const IMAGE_EXTENSIONS = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'heic', 'heif'])

export function sizeClass(file: Pick<File, 'name' | 'type'>): SizeClass {
  const type = file.type.split(';')[0].trim().toLowerCase()
  const dot = file.name.lastIndexOf('.')
  const extension = dot < 0 ? '' : file.name.slice(dot + 1).toLowerCase()
  if (type.startsWith('image/') || (!type && IMAGE_EXTENSIONS.has(extension))) return 'image'
  if (type === 'application/pdf' || (!type && extension === 'pdf')) return 'pdf'
  if (type.startsWith('text/') || TEXT_TYPES.has(type) || TEXT_EXTENSIONS.has(extension))
    return 'text'
  return 'other'
}

export interface AttachResult {
  files: PendingFile[]
  rejected: Rejected[]
}

/**
 * How a part is shown. `mime` is the server's (`mime_guess` on upload), which can be absent on an
 * older row: an unknown type is a plain file, never a broken `<img>`.
 */
export function mediaKind(mime: string | undefined | null): MediaKind {
  if (!mime) return 'file'
  if (mime.startsWith('image/')) return 'image'
  if (mime.startsWith('audio/')) return 'audio'
  if (mime.startsWith('video/')) return 'video'
  if (mime === 'application/pdf') return 'pdf'
  return 'file'
}

/** Only images get an inline preview in the composer: audio and video need a control, not a thumbnail. */
const previewable = (file: File) => mediaKind(file.type) === 'image'

const objectUrl = (file: File): string | undefined => {
  // jsdom and a worker have no createObjectURL; a missing preview falls back to the icon chip.
  if (typeof URL === 'undefined' || typeof URL.createObjectURL !== 'function') return undefined
  try {
    return URL.createObjectURL(file)
  } catch {
    return undefined
  }
}

/**
 * Add `incoming` to what is already attached, in the order chosen. Each file is judged against
 * the count, its own size and the running total, so a batch that is half acceptable attaches its
 * half and says what it dropped rather than failing whole.
 */
export function addFiles(
  current: PendingFile[],
  incoming: readonly File[],
  newId: () => string,
): AttachResult {
  const files = [...current]
  const rejected: Rejected[] = []
  let total = current.reduce((n, p) => n + p.file.size, 0)
  for (const file of incoming) {
    if (files.length >= tuning.MAX_ATTACHMENTS) {
      rejected.push({ name: file.name, reason: 'too-many' })
      continue
    }
    // A directory dragged onto the composer arrives as a zero-byte entry; so does an empty file.
    if (file.size === 0) {
      rejected.push({ name: file.name, reason: 'empty' })
      continue
    }
    const kind = sizeClass(file)
    if (file.size > tuning.ATTACHMENT_LIMITS[kind]) {
      rejected.push({ name: file.name, reason: 'too-large', kind })
      continue
    }
    if (total + file.size > tuning.ATTACHMENTS_MAX_TOTAL_BYTES) {
      rejected.push({ name: file.name, reason: 'over-budget' })
      continue
    }
    total += file.size
    files.push({ id: newId(), file, preview: previewable(file) ? objectUrl(file) : undefined })
  }
  return { files, rejected }
}

/** Drop one file, releasing its preview: the composer outlives many attach/remove rounds. */
export function removeFile(current: PendingFile[], id: string): PendingFile[] {
  const gone = current.find((p) => p.id === id)
  if (gone) releasePreview(gone)
  return current.filter((p) => p.id !== id)
}

export function releasePreview(file: PendingFile): void {
  if (file.preview && typeof URL?.revokeObjectURL === 'function') URL.revokeObjectURL(file.preview)
}

/** Release every preview at once (send, clear, unmount). */
export function releaseAll(files: readonly PendingFile[]): void {
  for (const f of files) releasePreview(f)
}

/** Where a saved part's bytes come from: one authenticated route, whatever the storage behind it. */
export const fileUrl = (id: string): string => `/api/chat/files/${encodeURIComponent(id)}/download`

/**
 * Placeholder parts for files a turn is sending, so its bubble can name them while they upload.
 * The ids are local and index-based: `userFilesPending` is what stops them reaching a download
 * link or a dispatch, and the saved parts replace these as soon as the upload returns.
 */
export const asFileParts = (files: readonly File[]): FilePart[] =>
  files.map((f, i) => ({ id: `pending-${i}`, name: f.name, size: f.size, mime: f.type }))
