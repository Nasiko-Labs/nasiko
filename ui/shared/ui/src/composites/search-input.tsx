/** A search field on shadcn `InputGroup`: magnifier addon + `type="search"` input. */
import { Search } from 'lucide-react'
import type { ComponentProps } from 'react'
import { InputGroup, InputGroupAddon, InputGroupInput } from '@nasiko/ui/primitives/input-group'
import { cn } from '@nasiko/ui/lib/utils'

export function SearchInput({
  className,
  ...props
}: Omit<ComponentProps<typeof InputGroupInput>, 'type'>) {
  return (
    <InputGroup className={cn('max-w-xs', className)}>
      <InputGroupAddon>
        <Search aria-hidden />
      </InputGroupAddon>
      <InputGroupInput type="search" {...props} />
    </InputGroup>
  )
}
