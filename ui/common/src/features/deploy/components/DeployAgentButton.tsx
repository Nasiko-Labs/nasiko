/**
 * The one "Deploy an agent" entry point (plans/feat-deploy.md §7): every CLI-only empty state gets it, and the CLI steps
 * stay beside it as the alternative. `name` pre-fills the form (an agent's "not deployed" state).
 */
import { Link } from '@tanstack/react-router'
import { Upload } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { copy } from '../copy'

export function DeployAgentButton({
  name,
  variant = 'default',
  size = 'sm',
  className,
}: {
  name?: string
  variant?: 'default' | 'outline'
  size?: 'sm' | 'default'
  className?: string
}) {
  return (
    <Button
      asChild
      variant={variant}
      size={size}
      className={cn('pointer-coarse:min-h-11', className)}
    >
      <Link to="/deploy" search={name ? { name } : {}}>
        <Upload aria-hidden /> {copy.entry.label}
      </Link>
    </Button>
  )
}
