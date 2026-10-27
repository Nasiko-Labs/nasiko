/** Pure formatting helpers shared by the agents pages (kept out of component files for fast refresh). */

/** The three commands from an empty server to a deployed agent (checked against `--help`). */
/** Deploys the project `nasiko new` scaffolded (`deploy` requires <IMAGE>: an image or agent directory). */
export const DEPLOY_CMD = 'nasiko deploy ./my-agent'

export function firstRunCommands(origin: string): string[] {
  return [`nasiko connect ${origin}`, 'nasiko new openai my-agent', DEPLOY_CMD]
}

const RTF = new Intl.RelativeTimeFormat('en', { numeric: 'auto' })

export function relTime(isoTime: string | null | undefined, now: number = Date.now()): string {
  if (!isoTime) return '—'
  const t = Date.parse(isoTime)
  if (Number.isNaN(t)) return '—'
  const s = Math.round((t - now) / 1000)
  const abs = Math.abs(s)
  if (abs < 60) return RTF.format(s, 'second')
  if (abs < 3600) return RTF.format(Math.round(s / 60), 'minute')
  if (abs < 86_400) return RTF.format(Math.round(s / 3600), 'hour')
  return RTF.format(Math.round(s / 86_400), 'day')
}
