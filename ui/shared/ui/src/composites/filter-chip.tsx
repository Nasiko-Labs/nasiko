import type { ReactNode } from 'react'
import { Toggle } from '@nasiko/ui/primitives/toggle'

export function FilterChip({
  active,
  onClick,
  children,
}: {
  active: boolean
  onClick: () => void
  children: ReactNode
}) {
  return (
    <Toggle
      variant="outline"
      size="sm"
      pressed={active}
      onPressedChange={onClick}
      className="min-h-8 rounded-full px-3 text-xs font-normal text-muted-foreground shadow-none hover:bg-transparent hover:text-foreground data-[state=on]:border-primary-text data-[state=on]:bg-primary/10 data-[state=on]:text-primary-text"
    >
      {children}
    </Toggle>
  )
}
