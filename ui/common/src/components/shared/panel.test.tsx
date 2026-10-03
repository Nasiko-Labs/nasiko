import { render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { ErrorState } from '@/features/observability/StateCard'
import { ApiError } from '@/lib/api/client'
import { PanelError } from './panel'

describe('403 is "No access", never a retryable failure', () => {
  it('PanelError says who to ask and offers no Retry', () => {
    render(
      <PanelError
        error={new ApiError(403, 'requires admin role', '/api/x', 'm')}
        onRetry={vi.fn()}
        what="spend"
      />,
    )
    expect(screen.getByText('No access to spend')).toBeInTheDocument()
    expect(screen.getByText(/requires admin role/)).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /retry/i })).toBeNull()
  })

  it('PanelError keeps Retry for a server failure', () => {
    render(
      <PanelError error={new ApiError(500, null, '/api/x', 'm')} onRetry={vi.fn()} what="spend" />,
    )
    expect(screen.getByText("Couldn't load spend")).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /retry/i })).toBeInTheDocument()
  })

  it('observability ErrorState renders No access', () => {
    render(<ErrorState error={new ApiError(403, null, '/api/x', 'm')} onRetry={vi.fn()} />)
    expect(screen.getByText("You don't have access to this.")).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /retry/i })).toBeNull()
  })
})
