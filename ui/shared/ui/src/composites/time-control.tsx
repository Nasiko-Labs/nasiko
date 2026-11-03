/**
 * Single time control (plan: replaces the old range-vs-month dual mode; A1b collapses
 * it to a select on narrow screens). `TimePresets` and `CustomRange` are its two halves, for a page that puts the
 * presets in the top bar (which is one fixed-height row) and the custom dates in the page.
 */
import { Input } from '@nasiko/ui/primitives/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@nasiko/ui/primitives/select'
import { ToggleGroup, ToggleGroupItem } from '@nasiko/ui/primitives/toggle-group'
import { PRESETS, type Preset } from '@nasiko/ui/lib/search'
import { cn } from '@nasiko/ui/lib/utils'

const LABELS: Record<Preset, string> = {
  '24h': '24h',
  '7d': '7d',
  '30d': '30d',
  mtd: 'This month',
  'last-month': 'Last month',
  custom: 'Custom',
}

type TimeControlProps = {
  /** The preset in the URL, not the resolved one: an invalid custom range keeps its inputs. */
  preset: Preset
  /** The custom range can't be used (inverted, impossible or in the future); the page shows 30d meanwhile. */
  invalid?: boolean
  from?: string
  to?: string
  /** UTC date, the latest selectable custom end. */
  today: string
  onChange: (next: { preset: Preset; from?: string; to?: string }) => void
}

export function TimeControl(props: TimeControlProps) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      <TimePresets {...props} />
      <CustomRange {...props} />
    </div>
  )
}

/** The presets: a segmented row, a select on narrow screens. */
export function TimePresets({ preset, from, to, today, onChange }: TimeControlProps) {
  const pick = (p: string) => {
    if (!p) return
    const next = p as Preset
    if (next === 'custom') onChange({ preset: 'custom', from: from ?? today, to: to ?? today })
    else onChange({ preset: next })
  }
  return (
    <>
      <div className="hidden md:block">
        <ToggleGroup
          variant="outline"
          size="sm"
          value={[preset]}
          onValueChange={(vs) => pick(vs[0] as Parameters<typeof pick>[0])}
          aria-label="Time window"
        >
          {PRESETS.map((p) => (
            <ToggleGroupItem key={p} value={p} className="px-2.5 text-xs">
              {LABELS[p]}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      </div>
      <div className="md:hidden">
        <Select value={preset} onValueChange={pick}>
          <SelectTrigger size="sm" aria-label="Time window" className="w-36">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {PRESETS.map((p) => (
              <SelectItem key={p} value={p}>
                {LABELS[p]}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
    </>
  )
}

/**
 * The custom range's From and To dates; nothing unless the preset is Custom. `noteElsewhere` drops the invalid-range
 * note for a one-line home (the top bar): the page renders `InvalidRangeNote` instead, which the To input still names.
 */
export function CustomRange({
  preset,
  invalid,
  from,
  to,
  today,
  onChange,
  noteElsewhere = false,
}: TimeControlProps & { noteElsewhere?: boolean }) {
  return preset === 'custom' ? (
    <div className="flex flex-wrap items-center gap-1 text-xs">
      <label className="sr-only" htmlFor="tw-from">
        From (UTC)
      </label>
      <Input
        id="tw-from"
        type="date"
        className="h-8 w-36"
        value={from ?? ''}
        max={to ?? today}
        onChange={(e) => onChange({ preset: 'custom', from: e.target.value, to })}
      />
      <span aria-hidden>–</span>
      <label className="sr-only" htmlFor="tw-to">
        To (UTC)
      </label>
      <Input
        id="tw-to"
        type="date"
        className="h-8 w-36"
        value={to ?? ''}
        min={from}
        max={today}
        onChange={(e) => onChange({ preset: 'custom', from, to: e.target.value })}
        aria-invalid={invalid || undefined}
        aria-describedby={invalid ? 'tw-invalid' : undefined}
      />
      {invalid && !noteElsewhere ? <InvalidRangeNote className="basis-full" /> : null}
    </div>
  ) : null
}

/** Why the page shows 30d although the URL asks for a custom range. */
export function InvalidRangeNote({ className }: { className?: string }) {
  return (
    <span id="tw-invalid" role="status" className={cn('text-sm text-warning', className)}>
      Invalid range, showing the last 30 days
    </span>
  )
}
