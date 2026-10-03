/** The edition's React glue (docs/lab-vs-react-migration-review.md §10.4): pages read the edition and its slots here. */
import { createContext, use } from 'react'
import { OSS, resolveSlots, type Edition, type EditionId, type Slots } from './edition'

export const EditionContext = createContext<{ edition: Edition; slots: Slots }>({
  edition: OSS,
  slots: resolveSlots([]),
})

/** The build's edition. Pages branch on this, never on whether a slot happens to be filled. */
export const useEdition = (): EditionId => use(EditionContext).edition.id

export const useSlots = (): Slots => use(EditionContext).slots
