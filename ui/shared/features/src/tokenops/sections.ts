/** TokenOps' page sections: their DOM ids and the one way to scroll to one (the section nav, `?open=`, the savings strip). */
import { prefersReducedMotion } from '@nasiko/ui/lib/useMediaQuery'

/** The DOM id of a section's `<section>`. */
export const sectionDomId = (id: string) => `tokenops-section-${id}`

export function scrollToSection(id: string) {
  const el = document.getElementById(sectionDomId(id))
  if (!el) return false
  el.scrollIntoView?.({ block: 'start', behavior: prefersReducedMotion() ? 'auto' : 'smooth' })
  return true
}
