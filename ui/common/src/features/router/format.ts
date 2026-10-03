/** Pure display helpers for the router page (kept out of component files for fast refresh). */
import { copy } from './copy'
import type { RoutingSource } from './types'

/** "Your default" to the owner, "Owner's default" to a superuser on someone else's agent. */
export function sourceLabel(source: RoutingSource, viewerIsOwner: boolean): string {
  if (source === 'attached') return copy.source.attached
  if (source === 'owner-default')
    return viewerIsOwner ? copy.source.default : copy.source.ownerDefault
  return copy.source.none
}
