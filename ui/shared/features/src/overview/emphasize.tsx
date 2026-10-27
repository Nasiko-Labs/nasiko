/** The headline's number emphasis, shared by the Overview headline and the home's summary sentence. */
import { Fragment } from 'react'

/** Money amounts, ranges and counts in semibold tabular figures. */
const NUMBER = /(\$[\d,.]+(?:[kKmM])?(?:–\$[\d,.]+(?:[kKmM])?)?|\b\d[\d,]*%?)/

export function emphasize(text: string) {
  // Keyed by offset and text: split alternates text and numbers, so no two parts share both.
  let at = 0
  return text.split(NUMBER).map((part, k) => {
    const key = `${at}:${part}`
    at += part.length
    return k % 2 ? (
      <span key={key} className="font-semibold tabular-nums">
        {part}
      </span>
    ) : (
      <Fragment key={key}>{part}</Fragment>
    )
  })
}
