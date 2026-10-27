/**
 * The CLI guide (`/cli`, the sidebar's help rows): install, the first steps against this cluster and a card per task
 * that opens its docs page. docs.nasiko.com can't be framed (X-Frame-Options: DENY), so the reference stays a link.
 */
import {
  ArrowUpRight,
  Bot,
  ChartNoAxesColumn,
  Hammer,
  MessageSquare,
  Rocket,
  Server,
  SquareTerminal,
} from 'lucide-react'
import { TopBar } from '@nasiko/app-core/shell/TopBar'
import { CopyButton } from '@nasiko/ui/composites/copy-button'
import { PageHeader } from '@nasiko/ui/composites/page-header'
import { asButton } from '@nasiko/ui/primitives/button'
import { Card } from '@nasiko/ui/primitives/card'
import { CLI_DOCS_URL, cliDocsUrl } from '@nasiko/app-core/links'
import { copy, TASKS } from './copy'

const ICONS = {
  setup: Server,
  'coding-agents': Bot,
  create: Hammer,
  operate: Rocket,
  chat: MessageSquare,
  observe: ChartNoAxesColumn,
} as const

/** This server's address for `nasiko connect`, or a placeholder where the page isn't served over http(s). */
const clusterUrl = () =>
  /^https?:$/.test(window.location.protocol) ? window.location.origin : 'https://<your-cluster>'

export function CliGuidePage() {
  const steps = [
    { label: copy.start.install, command: copy.install.unixCommand },
    { label: copy.start.connect, command: `nasiko connect ${clusterUrl()}` },
    { label: copy.start.signIn, command: 'nasiko auth login' },
    { label: copy.start.deploy, command: 'nasiko upload' },
  ]
  return (
    <div className="mx-auto flex w-full max-w-4xl flex-col gap-8">
      <TopBar root={copy.title} />
      <PageHeader
        title={
          <span className="flex items-center gap-3">
            <span className="flex size-10 items-center justify-center rounded-lg border bg-card">
              <SquareTerminal aria-hidden className="size-5" />
            </span>
            {copy.title}
          </span>
        }
        description={copy.sub}
        actions={
          <a
            href={CLI_DOCS_URL}
            target="_blank"
            rel="noopener noreferrer"
            {...asButton({ variant: 'outline', size: 'sm' })}
          >
            {copy.reference}
            <ArrowUpRight aria-hidden className="size-3.5" />{' '}
            <span className="sr-only">{copy.newTab}</span>
          </a>
        }
      />

      <section aria-labelledby="cli-install" className="flex flex-col gap-3">
        <h2 id="cli-install" className="text-base font-semibold">
          {copy.install.title}
        </h2>
        <p className="text-sm text-muted-foreground">{copy.install.line}</p>
        {[
          { label: copy.install.unix, command: copy.install.unixCommand },
          { label: copy.install.windows, command: copy.install.windowsCommand },
        ].map((c) => (
          <div key={c.label} className="flex flex-col gap-1.5">
            <span className="text-xs font-medium text-muted-foreground">{c.label}</span>
            <Command label={c.label} text={c.command} />
          </div>
        ))}
      </section>

      <section aria-labelledby="cli-tasks" className="flex flex-col gap-3">
        <h2 id="cli-tasks" className="text-base font-semibold">
          {copy.tasks.title}
        </h2>
        <ul className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {TASKS.map((t) => {
            const Icon = ICONS[t.page]
            return (
              <li key={t.page}>
                <Card className="relative h-full gap-2 p-4 transition-colors hover:bg-muted/40">
                  <Icon aria-hidden className="size-4 text-muted-foreground" />
                  <h3 className="text-sm font-semibold">
                    {/* The whole card is the link; the command below it stays selectable text. */}
                    <a
                      href={cliDocsUrl(t.page)}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="after:absolute after:inset-0 after:rounded-xl focus-visible:outline-none focus-visible:after:ring-2 focus-visible:after:ring-ring"
                    >
                      {t.title} <span className="sr-only">{copy.newTab}</span>
                    </a>
                  </h3>
                  <p className="text-xs text-muted-foreground">{t.line}</p>
                  <code className="mt-auto font-mono text-xs text-muted-foreground">
                    {t.command}
                  </code>
                </Card>
              </li>
            )
          })}
        </ul>
      </section>

      <section aria-labelledby="cli-start" className="flex flex-col gap-3">
        <h2 id="cli-start" className="text-base font-semibold">
          {copy.start.title}
        </h2>
        <ol className="flex flex-col gap-4">
          {steps.map((s, i) => (
            <li key={s.label} className="flex gap-3">
              <span
                aria-hidden
                className="flex size-6 shrink-0 items-center justify-center rounded-md border font-mono text-xs"
              >
                {i + 1}
              </span>
              <div className="flex min-w-0 flex-1 flex-col gap-1.5">
                <span className="text-sm">{s.label}</span>
                <Command label={s.label} text={s.command} />
              </div>
            </li>
          ))}
        </ol>
      </section>
    </div>
  )
}

/** A shell command with its copy button; long ones wrap rather than scroll. */
function Command({ text, label }: { text: string; label: string }) {
  return (
    <div className="flex items-center gap-2 rounded-md border bg-muted/50 py-1 pr-1 pl-3">
      <span aria-hidden className="font-mono text-xs text-muted-foreground select-none">
        $
      </span>
      <code className="min-w-0 flex-1 font-mono text-xs break-all whitespace-pre-wrap">{text}</code>
      <CopyButton text={text} label={copy.copyCommand(label)} />
    </div>
  )
}
