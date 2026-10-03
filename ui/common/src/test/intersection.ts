/**
 * A controllable IntersectionObserver for jsdom (v1b E-A6): `install()` replaces the global, and
 * `show(el)` / `showAll()` report elements as on screen, so lazy fetches can be counted.
 */
type Entry = { target: Element; isIntersecting: boolean; intersectionRatio: number }

class FakeObserver {
  static all = new Set<FakeObserver>()
  readonly targets = new Set<Element>()
  private readonly cb: (entries: Entry[], obs: FakeObserver) => void
  constructor(cb: (entries: Entry[], obs: FakeObserver) => void) {
    this.cb = cb
    FakeObserver.all.add(this)
  }
  observe(el: Element) {
    this.targets.add(el)
  }
  unobserve(el: Element) {
    this.targets.delete(el)
  }
  disconnect() {
    this.targets.clear()
    FakeObserver.all.delete(this)
  }
  takeRecords() {
    return []
  }
  fire(els: Element[]) {
    const hits = els.filter((e) => this.targets.has(e))
    if (hits.length)
      this.cb(
        hits.map((target) => ({ target, isIntersecting: true, intersectionRatio: 1 })),
        this,
      )
  }
}

export function installIntersectionObserver() {
  const prev = globalThis.IntersectionObserver
  globalThis.IntersectionObserver = FakeObserver as unknown as typeof IntersectionObserver
  return {
    show(...els: Element[]) {
      for (const o of [...FakeObserver.all]) o.fire(els)
    },
    showAll() {
      for (const o of [...FakeObserver.all]) o.fire([...o.targets])
    },
    observed: () => [...FakeObserver.all].reduce((n, o) => n + o.targets.size, 0),
    restore() {
      FakeObserver.all.clear()
      globalThis.IntersectionObserver = prev
    },
  }
}
