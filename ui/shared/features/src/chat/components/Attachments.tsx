/**
 * Attachments, in the two places they appear (plan §7.7): `AttachmentStrip` is what the composer
 * holds before a turn is sent, `MessageAttachments` is what a sent turn shows.
 *
 * Only the user's own attachments render as media. An assistant row's files stay download chips
 * (`FileChips` in `TurnView.tsx`): those bytes are an agent's workspace output, and this page's
 * standing rule is that agent-chosen content is never auto-rendered (see `Markdown.tsx`, which
 * turns an agent's images into labelled links for the same reason).
 */
import { FileAudio, FileText, FileVideo, ImageIcon, Paperclip, X } from 'lucide-react'
import { apiUrl } from '@nasiko/data-access/client'
import { Button } from '@nasiko/ui/primitives/button'
import { fmtBytes } from '@nasiko/ui/lib/format'
import { cn } from '@nasiko/ui/lib/utils'
import { fileUrl, mediaKind, type MediaKind, type PendingFile } from '../attachments'
import { copy } from '../copy'
import type { FilePart } from '../types'
import { TOUCH } from './turnStyles'

const ICONS = {
  image: ImageIcon,
  audio: FileAudio,
  video: FileVideo,
  pdf: FileText,
  file: FileText,
} satisfies Record<MediaKind, typeof FileText>

const CHIP =
  'inline-flex max-w-60 items-center gap-1.5 rounded-md border border-border bg-background px-2 py-1 text-xs'

function Icon({ kind }: { kind: MediaKind }) {
  const Glyph = ICONS[kind]
  return <Glyph className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
}

/** Name and size, the one label every attachment carries whatever it renders as. */
function Label({ name, size }: { name: string; size?: number }) {
  return (
    <>
      <span className="truncate">{name}</span>
      {size ? <span className="shrink-0 text-muted-foreground">{fmtBytes(size)}</span> : null}
    </>
  )
}

/** Chosen, not yet sent: a thumbnail for an image, an icon for everything else, each removable. */
export function AttachmentStrip({
  files,
  disabled,
  onRemove,
}: {
  files: readonly PendingFile[]
  disabled: boolean
  onRemove(id: string): void
}) {
  if (!files.length) return null
  return (
    <ul
      className="flex flex-wrap gap-2"
      aria-label={copy.attachedCount(files.length)}
      data-testid="attachment-strip"
    >
      {files.map((f) => (
        <li key={f.id} className={cn(CHIP, 'pr-1')}>
          {f.preview ? (
            // The local object URL of a file this user just chose; revoked on remove.
            <img src={f.preview} alt="" className="size-6 shrink-0 rounded object-cover" />
          ) : (
            <Icon kind={mediaKind(f.file.type)} />
          )}
          <Label name={f.file.name} size={f.file.size} />
          <Button
            type="button"
            size="icon-sm"
            variant="ghost"
            className="size-6 shrink-0 pointer-coarse:size-11"
            disabled={disabled}
            aria-label={copy.removeAttachment(f.file.name)}
            onClick={() => onRemove(f.id)}
          >
            <X className="size-3.5" aria-hidden />
          </Button>
        </li>
      ))}
    </ul>
  )
}

/**
 * A sent turn's attachments. `pending` means the ids are still local (the upload is in flight), so
 * nothing links anywhere yet — a download built from a placeholder id would 404.
 */
export function MessageAttachments({
  files,
  pending = false,
}: {
  files: readonly FilePart[]
  pending?: boolean
}) {
  if (!files.length) return null
  return (
    <div className="flex flex-col gap-2" data-testid="message-attachments">
      {files.map((f) => (
        <Attachment key={f.id} file={f} pending={pending} />
      ))}
      {pending ? <p className="text-xs text-muted-foreground">{copy.uploading}</p> : null}
    </div>
  )
}

function Attachment({ file, pending }: { file: FilePart; pending: boolean }) {
  const kind = mediaKind(file.mime)
  // Against the API base, not the page origin: the app and the API can be different hosts.
  const src = apiUrl(fileUrl(file.id)).href
  const label = <Label name={file.name} size={file.size} />
  if (pending)
    return (
      <span className={cn(CHIP, 'opacity-70')}>
        <Icon kind={kind} />
        {label}
      </span>
    )
  // Media plays or shows in place; the caption under it keeps the name and size (and, for a
  // format the browser won't take, is the way to get at the file at all).
  if (kind === 'image' || kind === 'audio' || kind === 'video')
    return (
      <figure className="flex flex-col gap-1">
        {kind === 'image' ? (
          <img
            src={src}
            alt={file.name}
            className="max-h-80 w-auto max-w-full rounded-md border border-border object-contain"
          />
        ) : kind === 'audio' ? (
          // eslint-disable-next-line jsx-a11y/media-has-caption -- a user's own upload has no track to offer
          <audio src={src} controls className="w-full max-w-md" />
        ) : (
          // eslint-disable-next-line jsx-a11y/media-has-caption -- a user's own upload has no track to offer
          <video src={src} controls className="max-h-80 w-auto max-w-full rounded-md" />
        )}
        <figcaption className="text-xs text-muted-foreground">
          <a href={src} download className="inline-flex items-center gap-1.5 hover:underline">
            <Paperclip className="size-3" aria-hidden />
            {label}
          </a>
        </figcaption>
      </figure>
    )
  return (
    <a href={src} download className={cn(CHIP, 'hover:bg-muted', TOUCH)}>
      <Icon kind={kind} />
      {label}
    </a>
  )
}
