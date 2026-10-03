/**
 * Confirm dialogs for destructive actions (plan §6.2). Radix restores focus to the trigger on
 * close. Actions stay disabled while their mutation is pending (no double submit).
 */
import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { useId, useState, type ReactNode } from 'react'
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { meQuery } from '@/lib/api/auth'
import { useDeleteAgent } from '../api'
import { copy, type ErrorContext } from '../copy'
import type { AgentView } from '../normalize'
import { ErrorNote } from './bits'

interface ConfirmProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  title: string
  body: ReactNode
  confirmLabel: string
  destructive?: boolean
  pending?: boolean
  error?: unknown
  errorContext?: ErrorContext
  onConfirm: () => void
  children?: ReactNode
  confirmDisabled?: boolean
}

export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  body,
  confirmLabel,
  destructive,
  pending,
  error,
  errorContext,
  onConfirm,
  children,
  confirmDisabled,
}: ConfirmProps) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{body}</AlertDialogDescription>
        </AlertDialogHeader>
        {children}
        {error ? <ErrorNote error={error} context={errorContext} /> : null}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>{copy.cancel}</AlertDialogCancel>
          {/* A plain Button, not AlertDialogAction: the dialog stays open until the call settles. */}
          <Button
            variant={destructive ? 'destructive' : 'default'}
            disabled={pending || confirmDisabled}
            onClick={onConfirm}
          >
            {pending ? `${confirmLabel}…` : confirmLabel}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

/** Delete: type the agent's unique `name` (not the display name) to enable the button. */
function DeleteDialog({
  open,
  onOpenChange,
  name,
  pending,
  error,
  onConfirm,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  name: string
  pending: boolean
  error: unknown
  onConfirm: () => void
}) {
  const [typed, setTyped] = useState('')
  const id = useId()
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={(o) => {
        if (!o) setTyped('')
        onOpenChange(o)
      }}
      title={copy.deleteTitle(name)}
      body={copy.deleteBody}
      confirmLabel={copy.deleteAgent}
      destructive
      pending={pending}
      error={error}
      errorContext="manage"
      confirmDisabled={typed !== name}
      onConfirm={onConfirm}
    >
      <label htmlFor={id} className="text-sm">
        {copy.deleteConfirmLead} <code className="font-mono">{name}</code> {copy.deleteConfirmTail}
      </label>
      <Input
        id={id}
        value={typed}
        onChange={(e) => setTyped(e.target.value)}
        autoComplete="off"
        spellCheck={false}
        className="font-mono"
      />
    </ConfirmDialog>
  )
}

export function RollbackDialog({
  open,
  onOpenChange,
  version,
  pending,
  error,
  onConfirm,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  version: string
  pending: boolean
  error: unknown
  onConfirm: (reason: string) => void
}) {
  const [reason, setReason] = useState('')
  const id = useId()
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={(o) => {
        if (!o) setReason('')
        onOpenChange(o)
      }}
      title={copy.rollBackTitle(version)}
      body={copy.rollBackBody}
      confirmLabel={copy.rollBackTo}
      pending={pending}
      error={error}
      errorContext="rollback"
      onConfirm={() => onConfirm(reason.trim())}
    >
      <label htmlFor={id} className="text-sm">
        {copy.rollBackReason}
      </label>
      <Textarea id={id} value={reason} onChange={(e) => setReason(e.target.value)} rows={2} />
    </ConfirmDialog>
  )
}

/**
 * Delete from anywhere on the detail page: the mutation, the type-the-name dialog and the
 * landing on Your agents with the result (replace: Back must not reopen a deleted agent).
 */
export function DeleteAgentDialog({
  agent,
  open,
  onOpenChange,
}: {
  agent: AgentView
  open: boolean
  onOpenChange: (open: boolean) => void
}) {
  const del = useDeleteAgent(agent.id)
  const navigate = useNavigate()
  const me = useQuery(meQuery).data
  return (
    <DeleteDialog
      open={open}
      onOpenChange={(o) => {
        if (o) del.reset()
        onOpenChange(o)
      }}
      name={agent.name}
      pending={del.isPending}
      error={del.isError ? del.error : undefined}
      onConfirm={() =>
        del.mutate(undefined, {
          onSuccess: (res) =>
            void navigate({
              to: '/agents/mine',
              replace: true,
              // A superuser deleting someone else's agent lands on that owner's list.
              search: agent.ownerId && agent.ownerId !== me?.sub ? { owner: agent.ownerId } : {},
              state: {
                agentDeleted: {
                  name: agent.displayName,
                  errors: res?.runtime_errors ?? [],
                  stopped: res?.containers_stopped ?? 0,
                },
              },
            }),
        })
      }
    />
  )
}
