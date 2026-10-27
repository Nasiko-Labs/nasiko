/**
 * The world picker: a small card in the canvas's bottom-right corner that redraws the village as another world. The
 * choice is saved with the village's other settings, and `?world=` in the link picks one too.
 */
import { ToggleGroup, ToggleGroupItem } from '@nasiko/ui/primitives/toggle-group'
import { cn } from '@nasiko/ui/lib/utils'
import { villageCopy as copy } from '../copy'
import { useVillagePrefs } from '../prefs'
import { CARD } from '../styles'
import { WORLD_IDS, WORLDS, type WorldId } from './index'

export function WorldPicker({ className }: { className?: string }) {
  const current = useVillagePrefs((s) => s.world)
  const set = useVillagePrefs((s) => s.set)
  return (
    <div data-vp-ignore="" className={cn(CARD, 'flex items-center gap-1 p-1 text-xs', className)}>
      <span aria-hidden className="px-1.5 font-medium text-muted-foreground">
        {copy.world.title}
      </span>
      <ToggleGroup
        size="sm"
        value={[current]}
        onValueChange={(vs) => {
          const id = vs[0] as string
          return id && set({ world: id as WorldId })
        }}
        aria-label={copy.world.label}
      >
        {WORLD_IDS.map((id) => {
          const w = WORLDS[id]
          return (
            <ToggleGroupItem
              key={id}
              value={id}
              title={w.blurb}
              className="h-7 gap-1.5 px-2 text-xs font-medium pointer-coarse:min-h-11"
            >
              <span
                aria-hidden
                className="size-3 rounded-full border border-black/20"
                style={{
                  background: `linear-gradient(135deg, ${w.swatch[0]} 50%, ${w.swatch[1]} 50%)`,
                }}
              />
              {w.label}
            </ToggleGroupItem>
          )
        })}
      </ToggleGroup>
    </div>
  )
}
