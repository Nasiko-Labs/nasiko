/**
 * The choice tile of Appearance and Optimization: the whole tile is the label (the click target), its picture takes the
 * ring when checked or focused. Shared so both pages keep one look (review advisory b).
 */
// items-stretch: the Label primitive centres its children, which would shrink each picture to its content.
export const TILE = 'group flex cursor-pointer flex-col items-stretch gap-2 text-sm font-normal'
export const PICTURE =
  'relative overflow-hidden rounded-lg border border-border transition-[border-color,box-shadow] group-hover:border-muted-foreground/50 group-has-[[data-state=checked]]:border-primary group-has-[[data-state=checked]]:ring-1 group-has-[[data-state=checked]]:ring-primary group-has-focus-visible:ring-2 group-has-focus-visible:ring-ring group-has-focus-visible:ring-offset-2 group-has-focus-visible:ring-offset-card motion-reduce:transition-none'
