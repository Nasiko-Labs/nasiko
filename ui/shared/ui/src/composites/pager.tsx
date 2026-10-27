import type { MouseEvent } from 'react'
import {
  Pagination,
  PaginationContent,
  PaginationEllipsis,
  PaginationItem,
  PaginationLink,
  PaginationNext,
  PaginationPrevious,
} from '@nasiko/ui/primitives/pagination'
import { pageWindow } from '@nasiko/ui/lib/paging'
import { cn } from '@nasiko/ui/lib/utils'

export function Pager({
  page,
  pages,
  onPage,
  className,
}: {
  page: number
  pages: number
  onPage: (page: number) => void
  className?: string
}) {
  if (pages <= 1) return null
  const go = (n: number) => (event: MouseEvent) => {
    event.preventDefault()
    onPage(Math.min(Math.max(0, n), pages - 1))
  }
  return (
    <Pagination className={cn('mx-0 w-auto justify-end', className)}>
      <PaginationContent>
        <PaginationItem>
          <PaginationPrevious
            href={`#page-${page}`}
            aria-disabled={page === 0}
            className={page === 0 ? 'pointer-events-none opacity-50' : undefined}
            onClick={go(page - 1)}
          />
        </PaginationItem>
        {pageWindow(page, pages).map((n) =>
          typeof n === 'string' ? (
            <PaginationItem key={n}>
              <PaginationEllipsis />
            </PaginationItem>
          ) : (
            <PaginationItem key={n}>
              <PaginationLink href={`#page-${n + 1}`} isActive={n === page} onClick={go(n)}>
                {n + 1}
              </PaginationLink>
            </PaginationItem>
          ),
        )}
        <PaginationItem>
          <PaginationNext
            href={`#page-${page + 2}`}
            aria-disabled={page === pages - 1}
            className={page === pages - 1 ? 'pointer-events-none opacity-50' : undefined}
            onClick={go(page + 1)}
          />
        </PaginationItem>
      </PaginationContent>
    </Pagination>
  )
}
