import { useNavigate } from '@tanstack/react-router'
import { toast } from 'sonner'
import { ConfirmDialog } from '@/features/agents/components/dialogs'
import { useDeleteConnector } from '../api'
import { copy } from '../copy'

/** Delete a server (legacy danger zone): revokes all agent access, then back to the catalog. */
export function DeleteDialog({
  id,
  label,
  open,
  onOpenChange,
}: {
  id: string
  label: string
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const navigate = useNavigate()
  const del = useDeleteConnector(id)
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      title={copy.deleteTitle(label)}
      body={copy.deleteBody}
      confirmLabel={copy.delete}
      destructive
      pending={del.isPending}
      error={del.error}
      onConfirm={() =>
        del.mutate(undefined, {
          onSuccess: () => {
            onOpenChange(false)
            toast.success(copy.deleted(label))
            void navigate({ to: '/mcp', search: {} })
          },
        })
      }
    />
  )
}
