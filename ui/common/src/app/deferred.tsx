import { lazy, Suspense, type ComponentType } from 'react'

/**
 * A slot component that loads when it first renders, not with the shell: the shell budget (200 KB gz) has no room
 * for a layer's own dependencies. Renders nothing until it arrives.
 */
export function deferred<P extends object>(
  load: () => Promise<ComponentType<P>>,
): ComponentType<P> {
  const Lazy = lazy(async () => ({ default: await load() }))
  return function Deferred(props: P) {
    return (
      <Suspense fallback={null}>
        <Lazy {...props} />
      </Suspense>
    )
  }
}
