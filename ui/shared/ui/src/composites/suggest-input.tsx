/**
 * A text field that suggests values, any text accepted: what a `<datalist>` did, in the theme's own popup (the browser
 * draws a datalist's, which no theme reaches; user 2026-11-06). Controlled: `value` is the text, and every keystroke
 * or picked suggestion calls `onValueChange`. The input's own props (id, placeholder, aria-*, onBlur, ref) pass through,
 * so a `<label htmlFor>` and react-hook-form's `Controller` field work as on an `Input`.
 */
import { useState, type ComponentProps } from 'react'
import {
  Autocomplete,
  useAutocompleteFilter,
  AutocompleteContent,
  AutocompleteInput,
  AutocompleteItem,
  AutocompleteList,
} from '../primitives/autocomplete'

export function SuggestInput({
  suggestions,
  value,
  onValueChange,
  ...input
}: {
  suggestions: readonly string[]
  value: string
  onValueChange: (value: string) => void
} & Omit<ComponentProps<typeof AutocompleteInput>, 'value' | 'defaultValue' | 'onChange'>) {
  // One entry per text: the same model can come from two providers.
  const items = [...new Set(suggestions)]
  const { contains } = useAutocompleteFilter()
  const [open, setOpen] = useState(false)
  return (
    <Autocomplete
      items={items}
      filter={contains}
      value={value}
      onValueChange={(v) => onValueChange(v)}
      // Open only with a suggestion to show: while the list is open Base UI hides the rest of the page from screen
      // readers, which an empty list would do for nothing.
      open={open && items.some((s) => contains(s, value))}
      onOpenChange={setOpen}
      openOnInputClick
    >
      <AutocompleteInput {...input} />
      <AutocompleteContent>
        <AutocompleteList>
          {(s: string) => (
            <AutocompleteItem key={s} value={s}>
              {s}
            </AutocompleteItem>
          )}
        </AutocompleteList>
      </AutocompleteContent>
    </Autocomplete>
  )
}
