import { Check } from 'lucide-react'

/** The check in the chosen tile's corner (shape, not just the ring's colour). */
export function Chosen() {
  return (
    <span
      aria-hidden
      className="absolute top-1.5 right-1.5 hidden size-4.5 items-center justify-center rounded-full bg-primary text-primary-foreground shadow-sm group-has-[[data-state=checked]]:flex"
    >
      <Check className="size-3" strokeWidth={3} />
    </span>
  )
}
