/**
 * Aceternity UI "Text Generate Effect" (registry item `@aceternity/text-generate-effect`, MIT): words arrive one after
 * another out of a blur.
 * Local edits:
 * - CSS, not Motion's `useAnimate`: each word runs `--animate-word-in` (the login headline's blur-in) with a staggered
 *   `animation-delay`, so no JS drives it ("CSS before JS").
 * - no wrapper, font or colour classes (Aceternity's `font-bold`, `dark:text-white text-black`): the words render into
 *   the caller's element and take its type and theme colour.
 * - the spaces sit between the word spans, so a screen reader and copy-paste read the plain sentence.
 * - still under reduced motion (`motion-reduce:animate-none`).
 * - replays when its parent remounts: give the parent a `key`.
 */
import { Fragment } from 'react'
import { enter } from '@nasiko/ui/lib/motion'

export function TextGenerateEffect({
  words,
  stagger = enter.wordStagger,
}: {
  words: string
  /** Seconds between two words. */
  stagger?: number
}) {
  return words.split(' ').map((word, i) => (
    // eslint-disable-next-line @eslint-react/no-array-index-key -- a sentence's words are positional, never reordered
    <Fragment key={i}>
      {i > 0 ? ' ' : null}
      <span
        className="inline-block animate-word-in motion-reduce:animate-none"
        style={{ animationDelay: `${i * stagger}s` }}
      >
        {word}
      </span>
    </Fragment>
  ))
}
