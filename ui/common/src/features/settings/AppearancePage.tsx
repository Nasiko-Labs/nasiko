/**
 * Settings → Appearance (moved here from the sidebar's Theme menu): mode (System, Light, Dark) and colour theme. Both
 * are this browser's preferences (`theme.ts`, applied before first paint by index.html), so every user has the page.
 * Each choice is a tile: a drawing of the app in that mode (or a button and nav tint in that theme's colour) over its
 * name. The radio stays in the tile for keyboard and screen readers (visually hidden); the chosen tile has a ring and
 * a check mark, so the choice never relies on colour alone.
 */
import { Check } from 'lucide-react'
import { useId, type CSSProperties, type ReactNode } from 'react'
import {
  ACCENTS,
  setAccent,
  setTheme,
  THEMES,
  useThemePrefs,
  type Accent,
  type Theme,
} from '@/app/shell/theme'
import { PageHeader } from '@/components/shared/page-header'
import { Label } from '@/components/ui/label'
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group'
import { cn } from '@/lib/utils'
import { SettingRow, SettingRows } from './components/SettingRow'
import { copy } from './copy'

/** The tile: the whole box is the label (the click target); its picture takes the ring when checked or focused. */
// items-stretch: the Label primitive centres its children, which would shrink each picture to its content.
const TILE = 'group flex cursor-pointer flex-col items-stretch gap-2 text-sm font-normal'
const PICTURE =
  'relative overflow-hidden rounded-lg border border-border transition-[border-color,box-shadow] group-hover:border-muted-foreground/50 group-has-[[data-state=checked]]:border-primary group-has-[[data-state=checked]]:ring-1 group-has-[[data-state=checked]]:ring-primary group-has-focus-visible:ring-2 group-has-focus-visible:ring-ring group-has-focus-visible:ring-offset-2 group-has-focus-visible:ring-offset-card motion-reduce:transition-none'

export function AppearancePage() {
  const prefs = useThemePrefs()
  const id = useId()
  return (
    <div className="flex flex-col gap-6">
      <PageHeader title={copy.appearance.label} description={copy.appearance.sub} />
      <SettingRows>
        <SettingRow
          label={copy.appearance.mode}
          hint={copy.appearance.modeHint}
          hintId={`${id}-mode`}
          stacked
        >
          <RadioGroup
            aria-label={copy.appearance.mode}
            aria-describedby={`${id}-mode`}
            value={prefs.theme}
            onValueChange={(v) => setTheme(v as Theme)}
            className="grid grid-cols-3 gap-3 @[560px]:gap-4"
          >
            {THEMES.map((t) => (
              <Label key={t.id} className={TILE}>
                <span className={cn(PICTURE, 'aspect-video')}>
                  {t.id === 'system' ? (
                    <>
                      <Window tone="light" />
                      {/* The dark half, cut on a diagonal. */}
                      <span className="absolute inset-0 [clip-path:polygon(60%_0,100%_0,100%_100%,40%_100%)]">
                        <Window tone="dark" />
                      </span>
                    </>
                  ) : (
                    <Window tone={t.id} />
                  )}
                  <Chosen />
                </span>
                <Caption>
                  <RadioGroupItem value={t.id} className="sr-only" />
                  {t.label}
                </Caption>
              </Label>
            ))}
          </RadioGroup>
        </SettingRow>
        <SettingRow
          label={copy.appearance.theme}
          hint={copy.appearance.themeHint}
          hintId={`${id}-theme`}
          stacked
        >
          <RadioGroup
            aria-label={copy.appearance.theme}
            aria-describedby={`${id}-theme`}
            value={prefs.accent}
            onValueChange={(v) => setAccent(v as Accent)}
            className="grid grid-cols-2 gap-3 @[560px]:grid-cols-4 @[560px]:gap-4"
          >
            {ACCENTS.map((a) => (
              <Label key={a.id} className={TILE}>
                <span
                  className={cn(
                    PICTURE,
                    'flex h-18 flex-col justify-center gap-2 bg-preview-light-side px-3',
                  )}
                  // The theme's own light primary (theme.ts), on the light drawing in either mode: each tile previews its theme.
                  style={{ '--swatch': a.swatch } as CSSProperties}
                >
                  <span className="flex h-3 w-4/5 items-center rounded-sm bg-[color-mix(in_oklab,var(--swatch)_16%,transparent)] px-1.5">
                    <span className="h-1 w-1/2 rounded-full bg-(--swatch)" />
                  </span>
                  <span className="flex items-center gap-1.5">
                    <span className="h-3.5 w-10 rounded-sm bg-(--swatch)" />
                    <span className="h-1 w-6 rounded-full bg-preview-light-ink" />
                  </span>
                  <Chosen />
                </span>
                <Caption>
                  <RadioGroupItem value={a.id} className="sr-only" />
                  {a.label}
                </Caption>
              </Label>
            ))}
          </RadioGroup>
        </SettingRow>
      </SettingRows>
    </div>
  )
}

function Caption({ children }: { children: ReactNode }) {
  return (
    <span className="px-0.5 text-muted-foreground group-has-[[data-state=checked]]:font-medium group-has-[[data-state=checked]]:text-foreground">
      {children}
    </span>
  )
}

/** The check in the chosen tile's corner (shape, not just the ring's colour). */
function Chosen() {
  return (
    <span
      aria-hidden
      className="absolute top-1.5 right-1.5 hidden size-4.5 items-center justify-center rounded-full bg-primary text-primary-foreground shadow-sm group-has-[[data-state=checked]]:flex"
    >
      <Check className="size-3" strokeWidth={3} />
    </span>
  )
}

/** A drawing of the app in one mode: sidebar with the current row tinted, a title, two cards and a button. */
function Window({ tone }: { tone: 'light' | 'dark' }) {
  const t =
    tone === 'light'
      ? {
          page: 'bg-preview-light-page',
          side: 'bg-preview-light-side border-preview-light-line',
          card: 'border-preview-light-line',
          ink: 'bg-preview-light-ink',
        }
      : {
          page: 'bg-preview-dark-page',
          side: 'bg-preview-dark-side border-preview-dark-line',
          card: 'border-preview-dark-line',
          ink: 'bg-preview-dark-ink',
        }
  return (
    <span aria-hidden className={cn('absolute inset-0 flex', t.page)}>
      <span className={cn('flex w-1/4 flex-col gap-1.5 border-r p-2', t.side)}>
        <span className={cn('h-1.5 w-3/4 rounded-full', t.ink)} />
        <span className="mt-1 h-2.5 w-full rounded-sm bg-primary/25" />
        <span className={cn('h-1.5 w-2/3 rounded-full', t.ink)} />
        <span className={cn('h-1.5 w-1/2 rounded-full', t.ink)} />
      </span>
      <span className="flex flex-1 flex-col gap-2 p-2.5">
        <span className={cn('h-2 w-2/5 rounded-full', t.ink)} />
        <span className="flex flex-1 gap-1.5">
          <span className={cn('flex-1 rounded-sm border', t.card)} />
          <span className={cn('flex-1 rounded-sm border', t.card)} />
        </span>
        <span className="h-2.5 w-1/4 self-end rounded-sm bg-primary" />
      </span>
    </span>
  )
}
