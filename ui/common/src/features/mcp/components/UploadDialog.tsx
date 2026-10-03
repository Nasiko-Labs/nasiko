/**
 * Upload MCP server (plans/feat-mcp.md §4, legacy upload modal): step 1 picks a zip or a GitHub repository, step 2 is
 * that method's form with Back. The name fills from the zip's file name (legacy rule, `nameFromFile`) until the user
 * types one. 202 closes the dialog: the server already wrote the `pending` row, so the refetched catalog shows the
 * building card, and a toast opens the server. Mounted only while open.
 */
import { useNavigate } from '@tanstack/react-router'
import { FolderGit2, Upload } from 'lucide-react'
import { useId, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog'
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import { ApiError } from '@/lib/api/client'
import { useUpload } from '../api'
import { copy, reason } from '../copy'
import { nameFromFile } from '../logic'

type Method = 'zip' | 'github'
type Problems = Partial<Record<'file' | 'name' | 'url', string>>

export function UploadDialog({ onClose }: { onClose: () => void }) {
  const id = useId()
  const navigate = useNavigate()
  const upload = useUpload()
  const [method, setMethod] = useState<Method | null>(null)
  const [file, setFile] = useState<File | null>(null)
  const [name, setName] = useState('')
  const [nameTouched, setNameTouched] = useState(false)
  const [version, setVersion] = useState('')
  const [url, setUrl] = useState('')
  const [problems, setProblems] = useState<Problems>({})

  const submit = (e: React.FormEvent) => {
    e.preventDefault()
    const p: Problems = {}
    if (!name.trim()) p.name = copy.required
    if (method === 'zip' && !file) p.file = copy.chooseZip
    if (method === 'github' && !/^https?:\/\//i.test(url.trim())) p.url = copy.badUrl
    setProblems(p)
    if (Object.keys(p).length || !method) return
    const common = { name: name.trim(), version: version.trim() || 'v1' }
    upload.mutate(
      method === 'zip' && file
        ? { kind: 'zip', ...common, file }
        : { kind: 'github', ...common, url: url.trim() },
      {
        onSuccess: (r) => {
          onClose()
          toast.success(copy.uploadQueued(common.name), {
            action: {
              label: copy.open,
              onClick: () =>
                void navigate({ to: '/mcp/$connectorId', params: { connectorId: r.connector_id } }),
            },
          })
        },
      },
    )
  }

  const noRights = upload.error instanceof ApiError && upload.error.status === 403

  return (
    <Dialog open onOpenChange={(o) => (o || upload.isPending ? null : onClose())}>
      <DialogContent className="sm:max-w-lg">
        {method === null ? (
          <>
            <DialogHeader>
              <DialogTitle>{copy.uploadTitle}</DialogTitle>
              <DialogDescription className="sr-only">{copy.uploadTitle}</DialogDescription>
            </DialogHeader>
            <div className="grid gap-3 sm:grid-cols-2">
              <MethodCard
                icon={<Upload className="size-5" aria-hidden />}
                title={copy.zipTitle}
                desc={copy.zipDesc}
                onPick={() => setMethod('zip')}
              />
              <MethodCard
                icon={<FolderGit2 className="size-5" aria-hidden />}
                title={copy.githubTitle}
                desc={copy.githubDesc}
                onPick={() => setMethod('github')}
              />
            </div>
          </>
        ) : (
          <form onSubmit={submit} noValidate className="flex flex-col gap-4">
            <DialogHeader>
              <DialogTitle>{method === 'zip' ? copy.zipTitle : copy.githubTitle}</DialogTitle>
              <DialogDescription>
                {method === 'zip' ? copy.zipDesc : copy.githubDesc}
              </DialogDescription>
            </DialogHeader>
            <FieldGroup className="gap-4">
              {method === 'zip' ? (
                <Field data-invalid={!!problems.file} className="gap-1.5">
                  <FieldLabel htmlFor={`${id}-file`}>{copy.file}</FieldLabel>
                  <Input
                    id={`${id}-file`}
                    type="file"
                    accept=".zip,application/zip"
                    aria-invalid={!!problems.file}
                    onChange={(e) => {
                      const f = e.target.files?.[0] ?? null
                      setFile(f)
                      setProblems((p) => ({ ...p, file: undefined }))
                      if (f && !nameTouched) setName(nameFromFile(f.name))
                    }}
                  />
                  {problems.file ? <FieldError>{problems.file}</FieldError> : null}
                </Field>
              ) : null}
              <Field data-invalid={!!problems.name} className="gap-1.5">
                <FieldLabel htmlFor={`${id}-name`}>{copy.uploadName.label}</FieldLabel>
                <Input
                  id={`${id}-name`}
                  autoComplete="off"
                  placeholder={copy.uploadName.placeholder}
                  value={name}
                  aria-invalid={!!problems.name}
                  onChange={(e) => {
                    setName(e.target.value)
                    setNameTouched(true)
                    setProblems((p) => ({ ...p, name: undefined }))
                  }}
                />
                {problems.name ? (
                  <FieldError>{problems.name}</FieldError>
                ) : method === 'zip' ? (
                  <FieldDescription>{copy.nameHint}</FieldDescription>
                ) : null}
              </Field>
              <Field className="gap-1.5">
                <FieldLabel htmlFor={`${id}-version`}>{copy.versionTag.label}</FieldLabel>
                <Input
                  id={`${id}-version`}
                  autoComplete="off"
                  placeholder={copy.versionTag.placeholder}
                  value={version}
                  onChange={(e) => setVersion(e.target.value)}
                />
              </Field>
              {method === 'github' ? (
                <Field data-invalid={!!problems.url} className="gap-1.5">
                  <FieldLabel htmlFor={`${id}-url`}>{copy.githubUrl.label}</FieldLabel>
                  <Input
                    id={`${id}-url`}
                    type="url"
                    autoComplete="off"
                    placeholder={copy.githubUrl.placeholder}
                    value={url}
                    aria-invalid={!!problems.url}
                    onChange={(e) => {
                      setUrl(e.target.value)
                      setProblems((p) => ({ ...p, url: undefined }))
                    }}
                  />
                  {problems.url ? <FieldError>{problems.url}</FieldError> : null}
                </Field>
              ) : null}
            </FieldGroup>
            {upload.isError ? (
              <p role="alert" className="text-sm text-destructive">
                {noRights ? copy.noDeployRights : copy.uploadFailed(reason(upload.error))}
              </p>
            ) : null}
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                disabled={upload.isPending}
                onClick={() => {
                  upload.reset()
                  setProblems({})
                  setMethod(null)
                }}
              >
                {copy.back}
              </Button>
              <Button type="submit" disabled={upload.isPending}>
                {upload.isPending ? copy.uploading : copy.uploadSubmit}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  )
}

/** A whole-card choice (legacy picker cards): the card is one button. */
function MethodCard({
  icon,
  title,
  desc,
  onPick,
}: {
  icon: React.ReactNode
  title: string
  desc: string
  onPick: () => void
}) {
  return (
    <Card asChild className="gap-2 p-4 text-left transition-colors hover:border-primary/40">
      <Button
        variant="ghost"
        className="h-auto flex-col items-start whitespace-normal"
        onClick={onPick}
      >
        <span className="text-primary-text">{icon}</span>
        <span className="font-medium">{title}</span>
        <span className="text-sm font-normal text-muted-foreground">{desc}</span>
      </Button>
    </Card>
  )
}
