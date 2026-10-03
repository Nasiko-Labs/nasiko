/**
 * Agent replies as Markdown (plan §6.12, D1): react-markdown + remark-gfm + rehype-sanitize,
 * no raw HTML and no dangerouslySetInnerHTML. Links open in a new tab; anything but http(s)
 * or mailto renders as text. Images never load: an agent-chosen src would be fetched as soon
 * as the reply renders (a zero-click leak), so they become a labelled link instead. Code blocks get Copy and Download; tables and code scroll inside
 * their own wrapper. While streaming, the text re-renders at most once per animation frame.
 */
import { Check, Copy, Download } from 'lucide-react'
import { memo, useEffect, useRef, useState, type ReactNode } from 'react'
import { safeHttpUrl } from '@/features/agents/normalize'
import { downloadText } from '@/lib/download'
import { useCopy } from '@/lib/useCopy'
import ReactMarkdown, { type Components } from 'react-markdown'
import rehypeSanitize from 'rehype-sanitize'
import remarkGfm from 'remark-gfm'
import { Button } from '@/components/ui/button'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { copy } from '../copy'
import { downloadName } from '../format'

const SAFE_HREF = /^(https?:|mailto:)/i

function CodeBlock({ lang, code }: { lang: string; code: string }) {
  const [state, onCopy] = useCopy(code)
  const copied = state === 'copied'
  return (
    <div className="my-2 overflow-hidden rounded-md border border-border bg-muted/40">
      <div className="flex items-center gap-1 border-b border-border px-2 py-1 text-xs text-muted-foreground">
        <span className="font-mono">{lang || 'text'}</span>
        <span className="ml-auto" />
        <Button
          size="xs"
          variant="ghost"
          className="min-h-8 pointer-coarse:min-h-11"
          onClick={() => void onCopy()}
          aria-label={copied ? copy.copied : state === 'failed' ? copy.copyFailed : copy.copyCode}
        >
          {copied ? <Check aria-hidden /> : <Copy aria-hidden />}
          {copied ? copy.copied : copy.copyReply}
        </Button>
        <Button
          size="xs"
          variant="ghost"
          className="min-h-8 pointer-coarse:min-h-11"
          onClick={() => downloadText(downloadName(lang), code)}
          aria-label={`${copy.download} ${downloadName(lang)}`}
        >
          <Download aria-hidden />
          {copy.download}
        </Button>
      </div>
      <pre className="overflow-x-auto p-3 text-xs leading-relaxed">
        <code>{code}</code>
      </pre>
    </div>
  )
}

const components: Components = {
  a: ({ href, children }) =>
    href && SAFE_HREF.test(href) ? (
      <a
        href={href}
        target="_blank"
        rel="noopener noreferrer"
        className="text-primary-text underline underline-offset-4"
      >
        {children}
      </a>
    ) : (
      <span>{children}</span>
    ),
  img: ({ src, alt }) => {
    const href = safeHttpUrl(typeof src === 'string' ? src : null)
    const label = copy.imageLink(alt || '')
    return href ? (
      <a
        href={href}
        target="_blank"
        rel="noopener noreferrer"
        className="text-primary-text underline underline-offset-4"
      >
        {label}
      </a>
    ) : (
      <span>{label}</span>
    )
  },
  pre: ({ children }) => <>{children}</>,
  code: ({ className, children }) => {
    const lang = /language-([\w+-]+)/.exec(className ?? '')?.[1]
    const text = String(children ?? '')
    // Fenced blocks carry a language class or end in a newline; inline code does neither.
    if (lang || text.endsWith('\n'))
      return <CodeBlock lang={lang ?? ''} code={text.replace(/\n$/, '')} />
    return <code className="rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]">{children}</code>
  },
  // shadcn Table parts; Table's own wrapper scrolls. Cells wrap: agent tables hold prose.
  table: ({ children }) => (
    <div className="my-2">
      <Table className="border-collapse">{children}</Table>
    </div>
  ),
  thead: ({ children }) => <TableHeader>{children}</TableHeader>,
  tbody: ({ children }) => <TableBody>{children}</TableBody>,
  tr: ({ children }) => <TableRow className="hover:bg-transparent">{children}</TableRow>,
  th: ({ children }) => (
    <TableHead className="h-auto border border-border px-2 py-1 whitespace-normal">
      {children}
    </TableHead>
  ),
  td: ({ children }) => (
    <TableCell className="border border-border px-2 py-1 align-top whitespace-normal">
      {children}
    </TableCell>
  ),
  ul: ({ children }) => <ul className="my-2 list-disc space-y-1 pl-5">{children}</ul>,
  ol: ({ children }) => <ol className="my-2 list-decimal space-y-1 pl-5">{children}</ol>,
  h1: ({ children }) => <h3 className="mt-3 mb-1 text-base font-semibold">{children}</h3>,
  h2: ({ children }) => <h3 className="mt-3 mb-1 text-base font-semibold">{children}</h3>,
  h3: ({ children }) => <h4 className="mt-3 mb-1 text-sm font-semibold">{children}</h4>,
  p: ({ children }) => <p className="my-2 leading-relaxed">{children}</p>,
  blockquote: ({ children }) => (
    <blockquote className="my-2 border-l-2 border-border pl-3 text-muted-foreground">
      {children}
    </blockquote>
  ),
}

const Rendered = memo(function Rendered({ text }: { text: string }) {
  return (
    <ReactMarkdown
      remarkPlugins={[remarkGfm]}
      rehypePlugins={[rehypeSanitize]}
      components={components}
      skipHtml
    >
      {text}
    </ReactMarkdown>
  )
})

/** Throttle a fast-changing string to one update per animation frame (and at most every 50 ms). */
function useFrameThrottled(text: string, live: boolean): string {
  const [shown, setShown] = useState(text)
  const latest = useRef(text)
  useEffect(() => {
    latest.current = text
  }, [text])
  useEffect(() => {
    if (!live) return
    let raf = 0
    let last = 0
    const tick = (t: number) => {
      if (t - last >= 50) {
        last = t
        setShown(latest.current)
      }
      raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [live])
  return live ? shown : text
}

export function Markdown({
  text,
  live = false,
  className,
}: {
  text: string
  live?: boolean
  className?: string
}): ReactNode {
  const shown = useFrameThrottled(text, live)
  return (
    <div className={className ?? 'text-sm break-words'}>
      <Rendered text={shown} />
    </div>
  )
}
