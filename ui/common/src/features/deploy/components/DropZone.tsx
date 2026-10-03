/**
 * The drop zone (design review 4, 14, 15): stock shadcn only. `Empty` is the dashed area, `Button` "Choose file" opens a
 * visually hidden `Input type="file"`; drag-and-drop on the `Empty` box is the only addition. On touch it's a 120 px
 * box with the button only (the drag hint is hidden there).
 */
import { FileArchive, X } from 'lucide-react'
import { forwardRef, useState, type DragEvent } from 'react'
import { Button } from '@/components/ui/button'
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
} from '@/components/ui/empty'
import { Input } from '@/components/ui/input'
import { fmtBytes } from '@/lib/format'
import { cn } from '@/lib/utils'
import { copy } from '../copy'

export const DropZone = forwardRef<
  HTMLButtonElement,
  {
    file: File | null
    disabled: boolean
    error?: string | null
    onFile: (f: File) => void
    onClear: () => void
  }
>(function DropZone({ file, disabled, error, onFile, onClear }, chooseRef) {
  const [over, setOver] = useState(false)
  const [inputKey, setInputKey] = useState(0)
  const onDrop = (e: DragEvent) => {
    e.preventDefault()
    setOver(false)
    const f = e.dataTransfer.files?.[0]
    if (f && !disabled) onFile(f)
  }
  const pick = (
    <label className="contents">
      <span className="sr-only">{copy.deploy.fileLabel}</span>
      <Input
        key={inputKey}
        type="file"
        accept=".zip,application/zip"
        className="sr-only"
        tabIndex={-1}
        data-testid="zip-input"
        disabled={disabled}
        onChange={(e) => {
          const f = e.target.files?.[0]
          if (f) onFile(f)
          // A new input, so choosing the same file again still fires.
          setInputKey((k) => k + 1)
        }}
      />
    </label>
  )
  return (
    <div className="flex flex-col gap-2">
      {!file ? (
        <Empty
          onDragOver={(e) => {
            e.preventDefault()
            if (!disabled) setOver(true)
          }}
          onDragLeave={() => setOver(false)}
          onDrop={onDrop}
          data-over={over || undefined}
          className={cn(
            'min-h-40 gap-3 border border-dashed px-4 py-6 pointer-coarse:min-h-30',
            over && 'border-primary bg-accent',
          )}
        >
          <EmptyHeader className="gap-1">
            <EmptyMedia variant="icon">
              <FileArchive aria-hidden />
            </EmptyMedia>
            <EmptyDescription className="text-sm pointer-coarse:hidden">
              {copy.deploy.drop}
            </EmptyDescription>
          </EmptyHeader>
          <EmptyContent className="gap-1">
            <Button
              ref={chooseRef}
              type="button"
              variant="outline"
              className="pointer-coarse:min-h-11"
              disabled={disabled}
              onClick={(e) =>
                (
                  e.currentTarget.parentElement?.querySelector(
                    'input[type=file]',
                  ) as HTMLInputElement | null
                )?.click()
              }
            >
              {copy.deploy.choose}
            </Button>
            {pick}
            <p className="text-xs text-muted-foreground">{copy.deploy.limit}</p>
          </EmptyContent>
        </Empty>
      ) : (
        <div className="flex items-center gap-3 rounded-md border px-3 py-2" data-testid="zip-file">
          <FileArchive aria-hidden className="size-4 shrink-0 text-muted-foreground" />
          <p className="min-w-0 flex-1 truncate text-sm">
            <span className="font-medium">{file.name}</span>{' '}
            <span className="text-muted-foreground">· {fmtBytes(file.size)}</span>
          </p>
          <Button
            type="button"
            variant="ghost"
            size="icon-sm"
            className="pointer-coarse:size-11"
            aria-label={copy.deploy.removeFile}
            disabled={disabled}
            onClick={onClear}
          >
            <X className="size-4" aria-hidden />
          </Button>
        </div>
      )}
      {error ? (
        <p className="text-xs text-destructive" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  )
})
