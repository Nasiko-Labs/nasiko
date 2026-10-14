// Ported from nasiko-ui-lab@1b99bdda common/src/components/shared/search-input.tsx. Local edits: imports mapped to
// ../primitives/input-group and ../lib/cn.
/** A search field on shadcn `InputGroup`: magnifier addon + `type="search"` input. */
import { Search } from "lucide-react";
import type { ComponentProps } from "react";
import { cn } from "../lib/cn";
import { InputGroup, InputGroupAddon, InputGroupInput } from "../primitives/input-group";

export function SearchInput({
  className,
  ...props
}: Omit<ComponentProps<typeof InputGroupInput>, "type">) {
  return (
    <InputGroup className={cn("max-w-xs", className)}>
      <InputGroupAddon>
        <Search aria-hidden />
      </InputGroupAddon>
      <InputGroupInput type="search" {...props} />
    </InputGroup>
  );
}
