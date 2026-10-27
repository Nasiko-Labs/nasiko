/** The hover store: chips that show on hover subscribe; a hover never re-renders the world. */
export function hoverStore() {
  let key: string | null = null
  const ls = new Set<() => void>()
  return {
    get: () => key,
    set(k: string | null) {
      if (k === key) return
      key = k
      ls.forEach((l) => l())
    },
    subscribe(l: () => void) {
      ls.add(l)
      return () => void ls.delete(l)
    },
  }
}
export type HoverStore = ReturnType<typeof hoverStore>
