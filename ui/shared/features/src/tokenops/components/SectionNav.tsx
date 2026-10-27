/**
 * The page's sections as one sticky row under the top bar: where you are on a long page, and one click to any part
 * of it. The section in view carries a pill that glides between items through a shared `layoutId`; the current item
 * is also `aria-current`, so position never relies on the pill alone.
 *
 * "In view" is the last section whose top has passed a line a third of the way down the screen, read on scroll
 * (passive, once per frame): three rects per frame, cheaper and simpler than an observer per threshold.
 */
import { m } from 'motion/react'
import { useEffect, useState } from 'react'
import { Button } from '@nasiko/ui/primitives/button'
import { transitions } from '@nasiko/ui/lib/motion'
import { cn } from '@nasiko/ui/lib/utils'
import { scrollToSection, sectionDomId } from '../sections'

export interface NavSection {
  id: string
  label: string
  hint?: string
}

export function SectionNav({ sections }: { sections: NavSection[] }) {
  const [active, setActive] = useState(sections[0]?.id)
  useEffect(() => {
    let frame = 0
    const read = () => {
      frame = 0
      const line = window.innerHeight / 3
      let current = sections[0]?.id
      for (const s of sections) {
        const top = document.getElementById(sectionDomId(s.id))?.getBoundingClientRect().top
        if (top !== undefined && top <= line) current = s.id
      }
      setActive(current)
    }
    const onScroll = () => {
      if (!frame) frame = requestAnimationFrame(read)
    }
    read()
    window.addEventListener('scroll', onScroll, { passive: true })
    return () => {
      window.removeEventListener('scroll', onScroll)
      if (frame) cancelAnimationFrame(frame)
    }
  }, [sections])
  return (
    <nav
      aria-label="Sections"
      className="sticky top-(--topbar-h) z-20 -mx-4 flex items-center gap-1 overflow-x-auto border-b bg-background px-4 py-2"
    >
      {sections.map((s) => (
        <Button
          key={s.id}
          variant="ghost"
          size="sm"
          aria-current={active === s.id ? 'true' : undefined}
          onClick={() => {
            setActive(s.id)
            scrollToSection(s.id)
          }}
          className={cn(
            'relative shrink-0 gap-1.5 text-muted-foreground hover:bg-transparent hover:text-foreground',
            active === s.id && 'text-foreground',
          )}
        >
          {active === s.id ? (
            <m.span
              layoutId="tokenops-section-pill"
              transition={transitions.morph}
              aria-hidden
              className="absolute inset-0 rounded-md bg-muted"
            />
          ) : null}
          <span className="relative">{s.label}</span>
          {s.hint ? (
            <span className="relative hidden text-xs font-normal text-muted-foreground lg:inline">
              {s.hint}
            </span>
          ) : null}
        </Button>
      ))}
    </nav>
  )
}
