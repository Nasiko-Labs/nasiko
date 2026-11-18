/**
 * The way to the free TokenOps assessment (`/assessment`): a one-line banner stuck above the top bar on every page
 * (user request 2026-11-05; it was a card in the sidebar). Its X hides it in this browser for good. It took the waitlist
 * card's place (WaitlistCta.tsx, now unused).
 */
import { Link } from '@tanstack/react-router'
import { ClipboardCheck, X } from 'lucide-react'
import { asButton, Button } from '@nasiko/ui/primitives/button'
import { copy } from '@nasiko/app-core/shell/copy'

/** One line, `--banner-h` tall (the shell sets it), so the pages' sticky rows can sit below it. */
export function AssessmentBanner({ onDismiss }: { onDismiss: () => void }) {
  return (
    <div
      role="region"
      aria-label={copy.assessment.title}
      className="flex h-(--banner-h) items-center gap-2 border-b border-border bg-muted px-2 text-sm md:px-4"
    >
      <span className="flex h-5.5 shrink-0 items-center gap-1.25 rounded-full bg-primary pr-2 pl-1.5 font-mono text-3xs font-medium tracking-[0.06em] text-primary-foreground uppercase">
        <ClipboardCheck aria-hidden className="size-3" />
        {copy.assessment.badge}
      </span>
      <p className="min-w-0 truncate">
        <span className="font-medium">{copy.assessment.title}</span>
        <span className="hidden text-muted-foreground lg:inline"> · {copy.assessment.line}</span>
      </p>
      <span className="flex-1" />
      <Link
        to="/assessment"
        {...asButton({ variant: 'outline', size: 'xs', className: 'shrink-0' })}
      >
        {copy.assessment.cta}
      </Link>
      <Button
        type="button"
        variant="ghost"
        size="icon-xs"
        aria-label={copy.assessment.dismiss}
        onClick={onDismiss}
        className="shrink-0 text-muted-foreground"
      >
        <X aria-hidden />
      </Button>
    </div>
  )
}
