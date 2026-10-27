"use client"

// Ported from nasiko-ui-lab@1b99bdda common/src/components/ui/slider.tsx (shadcn new-york-v4; the lab's edit: the
// thumb is `bg-background`). Local edits: Base UI `Slider` (Root › Control › Track › Indicator, Thumb) instead of
// Radix (Root › Track › Range, Thumb); Base UI marks orientation with `data-orientation` like Radix and disabled
// with `data-disabled`, so the lab's class strings carry over; `value`/`defaultValue` accept a number or an array
// in both; `cn` from "../lib/utils"; development (c96a2ba3a): `thumbLabel` / `thumbValueText` name the
// focusable thumb (axe `aria-input-field-name`), through Base UI's `getAriaLabel` / `getAriaValueText` on its input.
import { Slider as SliderPrimitive } from "@base-ui/react/slider"
import { cn } from "../lib/utils"
import * as React from "react"

/**
 * The thumb stays inside the track at min and max, as Radix's did: Radix's own offset (half the thumb, scaled from +1
 * at min to -1 at max) on top of Base UI's centring. Base UI's `thumbAlignment="edge"` measures the thumb instead,
 * and hides it until it has (so never under jsdom). A local edit (stage D); `HALF` is half of the thumb's `size-4`.
 */
const HALF = 8
function inBounds(state: SliderPrimitive.Thumb.State, index: number): React.CSSProperties {
  const span = state.max - state.min
  const at = span > 0 ? ((state.values[index] ?? state.min) - state.min) / span : 0
  const offset = `${HALF * (1 - 2 * at)}px`
  return state.orientation === "vertical"
    ? { marginBottom: offset }
    : { marginInlineStart: offset }
}

function Slider({
  className,
  defaultValue,
  value,
  min = 0,
  max = 100,
  thumbLabel,
  thumbValueText,
  ...props
}: SliderPrimitive.Root.Props & {
  /** Names the focusable thumb (it owns the slider role; a label on the root never reaches it). */
  thumbLabel?: string | undefined
  thumbValueText?: string | undefined
}) {
  const values = React.useMemo((): readonly number[] => {
    const pick = (v: unknown): readonly number[] | null =>
      Array.isArray(v)
        ? (v as readonly number[])
        : typeof v === "number"
          ? [v]
          : null
    return pick(value) ?? pick(defaultValue) ?? [min, max]
  }, [value, defaultValue, min, max])

  return (
    <SliderPrimitive.Root
      data-slot="slider"
      {...(defaultValue !== undefined ? { defaultValue } : {})}
      {...(value !== undefined ? { value } : {})}
      min={min}
      max={max}
      className={cn(
        "relative flex w-full touch-none items-center select-none data-[disabled]:opacity-50 data-[orientation=vertical]:h-full data-[orientation=vertical]:min-h-44 data-[orientation=vertical]:w-auto data-[orientation=vertical]:flex-col",
        className
      )}
      {...props}
    >
      <SliderPrimitive.Control className="relative flex w-full grow items-center data-[orientation=vertical]:h-full data-[orientation=vertical]:flex-col">
        <SliderPrimitive.Track
          data-slot="slider-track"
          className="relative grow overflow-hidden rounded-full bg-muted data-[orientation=horizontal]:h-1.5 data-[orientation=horizontal]:w-full data-[orientation=vertical]:h-full data-[orientation=vertical]:w-1.5"
        >
          <SliderPrimitive.Indicator
            data-slot="slider-range"
            className="absolute bg-primary data-[orientation=horizontal]:h-full data-[orientation=vertical]:w-full"
          />
        </SliderPrimitive.Track>
        {values.map((_, index) => (
          <SliderPrimitive.Thumb
            data-slot="slider-thumb"
            key={index}
            getAriaLabel={
              thumbLabel === undefined ? undefined : () => thumbLabel
            }
            getAriaValueText={
              thumbValueText === undefined ? undefined : () => thumbValueText
            }
            className="block size-4 shrink-0 rounded-full border border-primary bg-background shadow-sm ring-ring/50 transition-[color,box-shadow] hover:ring-4 focus-visible:ring-4 focus-visible:outline-hidden disabled:pointer-events-none disabled:opacity-50"
            style={(state) => inBounds(state, index)}
          />
        ))}
      </SliderPrimitive.Control>
    </SliderPrimitive.Root>
  )
}

export { Slider }
