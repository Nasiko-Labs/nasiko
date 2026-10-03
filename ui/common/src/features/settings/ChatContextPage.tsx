/**
 * Settings → Security → Chat context: which algorithm picks the history a chat request carries, and how much of it
 * (nasiko-cloud-rs `35c749af` ui/common/pages/chat-context-page.js; the React rebuild dropped it). Both are the
 * caller's own (`/api/me/context-strategy`, `/api/me/pacms-budget`), so every user has the page. One Save sends only
 * the changed values; the form is remounted after it (never `reset()`, see CLAUDE.md Settings).
 */
import { useId, useState } from 'react'
import { Controller, useForm } from 'react-hook-form'
import { toast } from 'sonner'
import { LeaveGuard } from '@/components/shared/leave-guard'
import { BetaBadge } from '@/components/shared/beta-badge'
import { PageHeader } from '@/components/shared/page-header'
import { PageLoader } from '@/components/shared/page-loader'
import { PanelError } from '@/components/shared/panel'
import { StateCard } from '@/components/shared/state-card'
import { Button } from '@/components/ui/button'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { ApiError } from '@/lib/api/client'
import { isEndpointAbsent } from '@/lib/api/detect'
import { useChatContext, useSaveChatContext, type ChatContext } from './api'
import { SettingRow, SettingRows } from './components/SettingRow'
import { copy as settingsCopy } from './copy'
import { BUDGET_LEVELS, CONTEXT_STRATEGIES } from './types'

const copy = settingsCopy.chatContext

export function ChatContextPage() {
  const ctx = useChatContext()
  const [saves, setSaves] = useState(0)
  return (
    <div className="flex flex-col gap-6">
      <PageHeader title={copy.label} description={copy.sub} actions={<BetaBadge />} />
      {ctx.isPending ? (
        <PageLoader label={copy.loading} />
      ) : ctx.isError ? (
        isEndpointAbsent(ctx.error) ? (
          <StateCard title={copy.newerServer} />
        ) : (
          <PanelError error={ctx.error} what={copy.loadWhat} onRetry={() => void ctx.refetch()} />
        )
      ) : (
        <ChatContextForm key={saves} saved={ctx.data} onSaved={() => setSaves((n) => n + 1)} />
      )}
    </div>
  )
}

function ChatContextForm({ saved, onSaved }: { saved: ChatContext; onSaved: () => void }) {
  const id = useId()
  const save = useSaveChatContext()
  const form = useForm<ChatContext>({ defaultValues: saved })
  const { isDirty } = form.formState
  const submit = form.handleSubmit((v) =>
    save.mutate(
      {
        strategy: v.strategy !== saved.strategy ? v.strategy : undefined,
        level: v.level !== saved.level ? v.level : undefined,
      },
      {
        onSuccess: () => {
          toast.success(copy.saved)
          onSaved()
        },
        onError: (err) =>
          toast.error(
            settingsCopy.saveFailed(
              (err instanceof ApiError && err.serverMessage) || (err as Error).message,
            ),
          ),
      },
    ),
  )
  const select = <K extends keyof ChatContext>(
    name: K,
    label: string,
    hint: string,
    values: readonly ChatContext[K][],
    labels: Record<ChatContext[K], string>,
  ) => (
    <SettingRow htmlFor={`${id}-${name}`} label={label} hint={hint} hintId={`${id}-${name}-hint`}>
      <Controller
        control={form.control}
        name={name}
        render={({ field }) => (
          <Select value={field.value} onValueChange={field.onChange}>
            <SelectTrigger
              id={`${id}-${name}`}
              aria-describedby={`${id}-${name}-hint`}
              className="w-full"
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {values.map((v) => (
                <SelectItem key={v} value={v}>
                  {labels[v]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
      />
    </SettingRow>
  )
  return (
    <form onSubmit={(e) => void submit(e)} noValidate>
      <SettingRows
        footer={
          <>
            {isDirty ? (
              <span className="mr-auto text-sm text-muted-foreground">{settingsCopy.unsaved}</span>
            ) : null}
            <Button
              type="submit"
              size="sm"
              className="pointer-coarse:min-h-11"
              disabled={!isDirty || save.isPending}
            >
              {save.isPending ? settingsCopy.saving : settingsCopy.save}
            </Button>
          </>
        }
      >
        {select('strategy', copy.strategy, copy.strategyHint, CONTEXT_STRATEGIES, copy.strategies)}
        {select('level', copy.budget, copy.budgetHint, BUDGET_LEVELS, copy.levels)}
      </SettingRows>
      <LeaveGuard when={isDirty && !save.isPending} />
    </form>
  )
}
