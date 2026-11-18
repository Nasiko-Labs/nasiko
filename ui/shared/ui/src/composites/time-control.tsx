/**
 * Single time control (plan: replaces the old range-vs-month dual mode; A1b collapses
 * it to a select on narrow screens). `TimePresets` and `CustomRange` are its two halves, for a page that puts the
 * presets in the top bar (which is one fixed-height row) and the custom dates in the page.
 */
import { useState } from 'react'
import { CalendarIcon } from 'lucide-react'
import { Button } from '@nasiko/ui/primitives/button'
import { Calendar } from '@nasiko/ui/primitives/calendar'
import { Popover, PopoverContent, PopoverTrigger } from '@nasiko/ui/primitives/popover'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@nasiko/ui/primitives/select'
import { ToggleGroup, ToggleGroupItem } from '@nasiko/ui/primitives/toggle-group'
import { fmtShortDay } from '@nasiko/ui/lib/format'
import { isRealDate, PRESETS, type Preset } from '@nasiko/ui/lib/search'
import { useMediaQuery } from '@nasiko/ui/lib/useMediaQuery'
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

/** A UTC day key as a date, and back: the picker runs in UTC (`timeZone`), like every day key the server sends. */
const utcDay = (key: string) => new Date(`${key}T00:00:00Z`)
const dayKey = (d: Date) => d.toISOString().slice(0, 10)
/** Two days as a range, earlier first. */
const span = (a: Date, b: Date) => (b < a ? { from: b, to: a } : { from: a, to: b })
/** The month and year dropdowns' first month (product call, NAS-1516). */
const EARLIEST_MONTH = utcDay('2025-01-01')

/**
 * The custom range's picker; nothing unless the preset is Custom. The first day clicked starts a range and the second
 * ends it (either order); only the whole range is saved (NAS-1516: native date inputs saved every keystroke, so typing
 * a year refetched with years like 0002, and their look and day order changed with the browser). Picking Custom opens
 * it; a link to a custom range doesn't. Between the clicks the range follows the pointer, and a line under the months
 * says which end comes next; two months from 48rem, the range's last month on the right, with month and year dropdowns
 * back to `EARLIEST_MONTH`. `noteElsewhere` drops the invalid-range note for a one-line home (the top bar): the page
 * renders `InvalidRangeNote` instead, which the button still names.
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
  const [open, setOpen] = useState(false)
  const [start, setStart] = useState<Date>()
  const [hover, setHover] = useState<Date>()
  const wide = useMediaQuery('(min-width: 48rem)')
  // Mounted before the preset changes, so only a switch to Custom opens it: a page loaded on Custom starts shut.
  const [seenPreset, setSeenPreset] = useState(preset)
  if (seenPreset !== preset) {
    setSeenPreset(preset)
    setOpen(preset === 'custom')
  }
  if (preset !== 'custom') return null
  const shown = (d?: string) => (d && isRealDate(d) ? fmtShortDay(d) : (d ?? '…'))
  const label = `${shown(from)} – ${shown(to)}`
  const saved = !invalid && from && to ? { from: utcDay(from), to: utcDay(to) } : undefined
  const end = utcDay(to && isRealDate(to) ? to : today)
  const reset = () => {
    setStart(undefined)
    setHover(undefined)
  }
  return (
    <div className="flex flex-wrap items-center gap-1 text-xs">
      <Popover
        open={open}
        onOpenChange={(next) => {
          setOpen(next)
          reset()
        }}
      >
        <PopoverTrigger
          render={
            <Button
              variant="outline"
              size="sm"
              className="h-8 gap-1.5 text-xs font-normal"
              aria-label={`Custom range (UTC): ${label}`}
              aria-invalid={invalid || undefined}
              aria-describedby={invalid ? 'tw-invalid' : undefined}
            />
          }
        >
          <CalendarIcon aria-hidden />
          {label}
        </PopoverTrigger>
        <PopoverContent align="start" className="w-auto p-0">
          <Calendar
            mode="range"
            timeZone="UTC"
            captionLayout="dropdown"
            showOutsideDays={false}
            numberOfMonths={wide ? 2 : 1}
            today={utcDay(today)}
            selected={start ? span(start, hover ?? start) : saved}
            defaultMonth={
              wide ? new Date(Date.UTC(end.getUTCFullYear(), end.getUTCMonth() - 1)) : end
            }
            startMonth={EARLIEST_MONTH}
            endMonth={utcDay(today)}
            disabled={{ after: utcDay(today) }}
            onDayMouseEnter={(day, m) => !m.disabled && setHover(day)}
            onSelect={(_, day) => {
              if (!start) return setStart(day)
              const range = span(start, day)
              setOpen(false)
              reset()
              onChange({ preset: 'custom', from: dayKey(range.from), to: dayKey(range.to) })
            }}
          />
          <p aria-live="polite" className="border-t px-3 py-2 text-xs text-muted-foreground">
            {start
              ? `${fmtShortDay(dayKey(start))} – now pick the last day`
              : 'Pick the first and last day (UTC)'}
          </p>
        </PopoverContent>
      </Popover>
      {invalid && !noteElsewhere ? <InvalidRangeNote className="basis-full" /> : null}
    </div>
  )
}

/** Why the page shows 30d although the URL asks for a custom range. */
export function InvalidRangeNote({ className }: { className?: string }) {
  return (
    <span id="tw-invalid" role="status" className={cn('text-sm text-warning', className)}>
      Invalid range, showing the last 30 days
    </span>
  )
}
