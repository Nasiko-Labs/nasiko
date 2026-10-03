/**
 * Access (plan §7.3, managers only): public toggle (waits for the server), user grants with
 * username search, any grantee kinds an edition layer adds (`granteeKinds`; EE: org units), and the agent-to-agent
 * list. Lists load and fail independently. Both editions' `/grants` shapes are read here (grants.ts): EE serves its
 * own shapes at the same paths, and this tab renders in the EE build too.
 * The agent list means opposite things by edition (grants.ts). OSS: the agents this one may call, recorded but not
 * enforced at ea233d20 (the tab says so). EE: the agents allowed to call this one; grants only add access. Neither
 * blocks anything, so nothing asks for confirmation.
 */
import { useId, useState } from 'react'
import { useSlots } from '@/app/edition-context'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Field, FieldLabel } from '@/components/ui/field'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Skeleton } from '@/components/ui/skeleton'
import {
  useAgentGrants,
  useAgentsDirectory,
  useGrantMutations,
  useGrants,
  useUserGrants,
  useUserSearch,
} from '../api'
import { ErrorNote, LearnMore, Section } from '../components/bits'
import { copy } from '../copy'
import type { AgentView } from '../normalize'
import { isHarness } from '../status'
import { USER_SEARCH_MIN } from '../tuning'

export function AccessTab({ agent }: { agent: AgentView }) {
  const grants = useGrants(agent.id, true)
  const users = useUserGrants(agent.id, true)
  const acl = useAgentGrants(agent.id, true)
  const m = useGrantMutations(agent.id)
  const dir = useAgentsDirectory()
  const publicId = useId()
  const { granteeKinds } = useSlots()
  const ee = grants.data?.edition === 'ee'

  return (
    <div className="space-y-4">
      <Section title={copy.access} action={<LearnMore href="access" />}>
        {grants.isPending ? (
          <Skeleton className="h-8" />
        ) : grants.isError ? (
          <ErrorNote error={grants.error} onRetry={() => void grants.refetch()} />
        ) : (
          <Field orientation="horizontal" className="gap-2">
            <Checkbox
              id={publicId}
              checked={grants.data.is_public}
              disabled={m.setPublic.isPending}
              onCheckedChange={(v) => m.setPublic.mutate(v === true)}
            />
            <FieldLabel htmlFor={publicId} className="font-normal">
              {copy.publicToggle}
            </FieldLabel>
          </Field>
        )}
        {m.setPublic.isError ? <ErrorNote error={m.setPublic.error} context="manage" /> : null}
      </Section>

      <Section title={copy.userGrants}>
        {users.isPending ? (
          <Skeleton className="h-10" />
        ) : users.isError ? (
          <ErrorNote error={users.error} onRetry={() => void users.refetch()} />
        ) : users.data.length ? (
          <ul className="divide-y divide-border text-sm">
            {users.data.map((u) => (
              <li key={u.user_id} className="flex items-center justify-between py-1.5">
                <span className={u.username ? undefined : 'font-mono text-xs'}>
                  {u.username ?? u.user_id}
                </span>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={m.removeUser.isPending}
                  aria-label={copy.removeNamed(u.username ?? u.user_id)}
                  onClick={() => m.removeUser.mutate(u.user_id)}
                >
                  {copy.remove}
                </Button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-sm text-muted-foreground">{copy.noUserGrants}</p>
        )}
        {/* Adds wait for /grants: the edition decides the write form (grants.ts). */}
        {grants.data ? (
          <UserPicker
            exclude={new Set([agent.ownerId, ...(users.data ?? []).map((u) => u.user_id)])}
            pending={m.addUser.isPending}
            onPick={(id) => m.addUser.mutate(id)}
          />
        ) : null}
        {m.addUser.isError ? <ErrorNote error={m.addUser.error} context="manage" /> : null}
        {m.removeUser.isError ? <ErrorNote error={m.removeUser.error} context="manage" /> : null}
      </Section>

      {/* Other grantees an edition layer adds (EE: org units), each in its own section. */}
      {granteeKinds.map((k) => (
        <Section key={k.key} title={k.label}>
          <k.Section agentId={agent.id} />
        </Section>
      ))}

      <Section title={ee ? copy.agentAclEe : copy.agentAcl}>
        {acl.isPending ? (
          <Skeleton className="h-10" />
        ) : acl.isError ? (
          <ErrorNote error={acl.error} onRetry={() => void acl.refetch()} />
        ) : acl.data.length ? (
          <ul className="divide-y divide-border text-sm">
            {acl.data.map((g) => (
              <li key={g.target_agent_id} className="flex items-center justify-between py-1.5">
                <span className="font-mono text-xs">
                  {g.target_name ?? dir.byId.get(g.target_agent_id)?.name ?? g.target_agent_id}
                </span>
                <Button
                  size="sm"
                  variant="ghost"
                  disabled={m.removeAgent.isPending}
                  aria-label={copy.removeNamed(
                    g.target_name ?? dir.byId.get(g.target_agent_id)?.name ?? g.target_agent_id,
                  )}
                  onClick={() => m.removeAgent.mutate(g.target_agent_id)}
                >
                  {copy.remove}
                </Button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-sm text-muted-foreground">{ee ? copy.noAclEe : copy.noAcl}</p>
        )}
        {grants.data && !ee ? (
          <p className="text-xs text-muted-foreground">{copy.aclNotEnforced}</p>
        ) : null}
        {grants.data ? (
          <AgentPicker
            options={(dir.data ?? []).filter(
              (a) =>
                a.id !== agent.id &&
                !isHarness(a) &&
                !(acl.data ?? []).some((g) => g.target_agent_id === a.id),
            )}
            pending={m.addAgent.isPending}
            onPick={(id) => m.addAgent.mutate(id)}
          />
        ) : null}
        {m.addAgent.isError ? <ErrorNote error={m.addAgent.error} context="manage" /> : null}
        {m.removeAgent.isError ? <ErrorNote error={m.removeAgent.error} context="manage" /> : null}
      </Section>
    </div>
  )
}

function UserPicker({
  exclude,
  pending,
  onPick,
}: {
  exclude: ReadonlySet<string>
  pending: boolean
  onPick: (id: string) => void
}) {
  const [q, setQ] = useState('')
  const search = useUserSearch(q)
  const hits = (search.data ?? []).filter((u) => !exclude.has(u.id))
  const short = q.trim().length < USER_SEARCH_MIN
  return (
    <div className="space-y-2 pt-2">
      <Field className="max-w-sm gap-2">
        <FieldLabel htmlFor="grant-user-search">{copy.addUser}</FieldLabel>
        <Input
          id="grant-user-search"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder={copy.searchUsers}
          autoComplete="off"
        />
      </Field>
      {q && short ? (
        <p className="text-xs text-muted-foreground">{copy.typeMore(USER_SEARCH_MIN)}</p>
      ) : null}
      {!short && search.isError ? <ErrorNote error={search.error} /> : null}
      {!short && search.isSuccess && !hits.length ? (
        <p className="text-xs text-muted-foreground">{copy.noUsers}</p>
      ) : null}
      {hits.length ? (
        <ul className="max-w-sm divide-y divide-border rounded-md border border-border text-sm">
          {hits.map((u) => (
            <li key={u.id} className="flex items-center justify-between px-2 py-1">
              <span>
                {u.display_name || u.username}{' '}
                <span className="text-xs text-muted-foreground">{u.username}</span>
              </span>
              <Button
                size="sm"
                variant="outline"
                disabled={pending}
                aria-label={copy.addNamed(u.display_name || u.username)}
                onClick={() => {
                  onPick(u.id)
                  setQ('')
                }}
              >
                {copy.add}
              </Button>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  )
}

function AgentPicker({
  options,
  pending,
  onPick,
}: {
  options: { id: string; name: string; display_name?: string | null }[]
  pending: boolean
  onPick: (id: string) => void
}) {
  const [id, setId] = useState('')
  const fieldId = useId()
  return (
    <div className="flex flex-wrap items-end gap-2 pt-2">
      <Field className="w-auto gap-1">
        <FieldLabel htmlFor={fieldId}>{copy.addAgent}</FieldLabel>
        {/* value "" shows the placeholder (Radix Select); it resets after each Allow. */}
        <Select value={id} onValueChange={setId}>
          <SelectTrigger id={fieldId} className="min-w-56">
            <SelectValue placeholder={copy.chooseAgent} />
          </SelectTrigger>
          <SelectContent>
            {options.map((a) => (
              <SelectItem key={a.id} value={a.id}>
                {a.display_name || a.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </Field>
      <Button
        size="sm"
        variant="outline"
        disabled={!id || pending}
        onClick={() => {
          onPick(id)
          setId('')
        }}
      >
        {copy.allow}
      </Button>
    </div>
  )
}
