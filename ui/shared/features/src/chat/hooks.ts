/** Chat's small hooks: a ticking clock, a draft mirrored to drafts.ts, and the composer's attachments. */
import { useCallback, useEffect, useRef, useState } from 'react'
import { fmtBytes } from '@nasiko/ui/lib/format'
import { uuid } from '@nasiko/ui/lib/utils'
import { addFiles, releaseAll, removeFile, type PendingFile, type Rejected } from './attachments'
import { copy } from './copy'
import { readDraft, writeDraft } from './drafts'
import { tuning } from './tuning'

/**
 * The time, re-read every `intervalMs`: only a component that shows elapsed or relative time ticks. `fixed`
 * pins it (tests), and then nothing ticks.
 */
export function useNow(intervalMs: number, fixed?: number): number {
  const [now, setNow] = useState(() => Date.now())
  const ticking = fixed === undefined
  useEffect(() => {
    if (!ticking) return
    const id = setInterval(() => setNow(Date.now()), intervalMs)
    return () => clearInterval(id)
  }, [intervalMs, ticking])
  return fixed ?? now
}

/** Draft state for one chat key, mirrored to drafts.ts on every change. */
export function useDraft(userId: string, key: string) {
  const [state, setState] = useState(() => ({ key, text: readDraft(userId, key) }))
  // The key can change after mount (on a cold load `?agent=` resolves once the directory
  // loads): read that key's stored draft instead of keeping, then overwriting, the old one.
  if (state.key !== key) setState({ key, text: readDraft(userId, key) })
  const setDraft = useCallback(
    (v: string) => {
      setState({ key, text: v })
      writeDraft(userId, key, v)
    },
    [userId, key],
  )
  return [state.key === key ? state.text : readDraft(userId, key), setDraft] as const
}

/** One line saying which file was turned away and why (§7.7). */
export function rejectedText(r: Rejected): string {
  const kind = r.kind ?? 'other'
  switch (r.reason) {
    case 'too-many':
      return copy.attachTooMany(r.name, tuning.MAX_ATTACHMENTS)
    case 'too-large':
      return copy.attachTooLarge(
        r.name,
        copy.attachKinds[kind],
        fmtBytes(tuning.ATTACHMENT_LIMITS[kind]),
      )
    case 'over-budget':
      return copy.attachOverBudget(r.name, fmtBytes(tuning.ATTACHMENTS_MAX_TOTAL_BYTES))
    case 'empty':
      return copy.attachEmpty(r.name)
  }
}

/**
 * The composer's attachments for one chat (§7.7). Held by the view, not the registry: the list
 * must survive a refused send, and the object-URL previews are revoked here — on remove, when a
 * send takes them, and on unmount.
 */
export function useAttachments() {
  const [files, setFiles] = useState<PendingFile[]>([])
  const [rejected, setRejected] = useState<string[]>([])
  // The committed list, readable from cleanup and from `take` without making either depend on
  // it. Written in an effect, never during render: both readers run after a commit.
  const latest = useRef(files)
  useEffect(() => {
    latest.current = files
  }, [files])
  // Unmount only (leaving the chat, a user switch): a preview must not outlive its page.
  useEffect(() => () => releaseAll(latest.current), [])
  const onAdd = useCallback((incoming: readonly File[]) => {
    setFiles((current) => {
      const result = addFiles(current, incoming, uuid)
      setRejected(result.rejected.map(rejectedText))
      return result.files
    })
  }, [])
  const onRemove = useCallback((id: string) => {
    setRejected([])
    setFiles((current) => removeFile(current, id))
  }, [])
  /** Give the chosen files to a send and clear the strip; their previews go with them. */
  const take = useCallback((): File[] => {
    const taken = latest.current
    releaseAll(taken)
    setFiles([])
    setRejected([])
    return taken.map((p) => p.file)
  }, [])
  /** Put them back after a send was refused before anything was dispatched. */
  const restore = useCallback((back: readonly File[]) => {
    setFiles(addFiles([], back, uuid).files)
  }, [])
  return { files, rejected, onAdd, onRemove, take, restore }
}
