/**
 * `/chat` (plan §7, v1b §5.1, v1c §5.2): a new chat, its target from `?agent=`, `?auto=1` or the remembered
 * choice. The first send creates the chat, then moves to it.
 */
import { useQueryClient } from '@tanstack/react-query'
import { Link, useNavigate, useRouter } from '@tanstack/react-router'
import { ChevronDown } from 'lucide-react'
import { memo, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { PageLoader } from '@/components/shared/page-loader'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { useAgentsDirectory } from '@/features/agents/api'
import { chatKeys, removeSessionRow, sessionRows, upsertSessionRow, useChatSessions } from './api'
import { Composer, type ComposerHandle, type ComposerMode } from './components/Composer'
import {
  AgentChooser,
  ExampleChips,
  NewChatHero,
  RecentChats,
  TargetBanner,
  TargetList,
} from './components/NewChat'
import { TargetPicker, type TargetChoice } from './components/TargetPicker'
import { StatusAnnouncer } from './components/StatusAnnouncer'
import { TurnView, type TurnHandlers } from './components/TurnView'
import { copy } from './copy'
import {
  carryDraft,
  dropUndo,
  newChatDraftKey,
  pendingUndo,
  readDraft,
  undoCarry,
  writeDraft,
} from './drafts'
import { announce } from './announce'
import { focusIfIdle } from './focus'
import { agentLabel, chatKind } from './format'
import { useLiveTurn, useTitlePrefix } from './registry'
import { CHAT_COLUMN } from './components/turnStyles'
import { chatIdentity, targetIdentity } from './identity'
import { readRememberedTarget, rememberTarget } from './rememberTarget'
import { isAuto } from './search'
import {
  agentExamples,
  chooserAgents,
  orchestratorExamples,
  preselectTarget,
  resolveAgentParam,
} from './target'
import type { DisplayTurn } from './turnModel'
import { isLivePhase, type LiveTurn, type SendInput } from './turnRegistry'
import type { ChatSessionRow } from './types'
import { useDraft } from './hooks'
import { directoryOf } from './target'
import { sendErrorText } from './errors'
import { ConfirmDialog, NotRunningBanner, SendError, type ViewProps } from './components/PageParts'
import { DebugFrames, UnknownScenarioBanner } from './components/DebugPanels'

function NewChatView({ userId, registry, search, railButton, username }: ViewProps) {
  const client = useQueryClient()
  const router = useRouter()
  const navigate = useNavigate()
  const dir = useAgentsDirectory()
  const list = useChatSessions()
  const target = resolveAgentParam(search.agent, directoryOf(dir), isAuto(search))
  const agent = target.kind === 'direct' ? target.agent : undefined
  const routed = target.kind === 'routed'
  const noTarget = target.kind === 'choose'
  const draftKeyOf = (a: { id: string } | undefined) =>
    newChatDraftKey(routed ? { routed: true } : { agentId: a?.id })
  const [pendingId, setPendingId] = useState<string>()
  const live = useLiveTurn(registry, pendingId)
  const [draft, setDraftRaw] = useDraft(userId, draftKeyOf(agent))
  const [sendError, setSendError] = useState<unknown>(null)
  const [replaceWith, setReplaceWith] = useState<string | null>(null)
  const [pickerOpen, setPickerOpen] = useState(false)
  const titlePrefix = useTitlePrefix()
  // A target choice that displaced a saved draft offers Undo here (DS-T1, E15). This view remounts per
  // target, so the offer is read once from drafts.ts.
  const [undo, setUndo] = useState(() => pendingUndo(userId, draftKeyOf(agent)))
  useEffect(() => {
    if (!undo) return
    const t = setTimeout(() => setUndo(null), Math.max(0, undo.expiresAt - Date.now()))
    return () => clearTimeout(t)
  }, [undo])
  /** Any edit ends the Undo offer (E15). */
  const setDraft = (v: string) => {
    if (undo) {
      dropUndo(userId)
      setUndo(null)
    }
    setDraftRaw(v)
  }
  const composer = useRef<ComposerHandle>(null)
  const carry = { mock: search.mock, debug: search.debug }
  const mounted = useRef(true)
  useEffect(() => {
    mounted.current = true
    return () => {
      mounted.current = false
    }
  }, [])
  // Focus the composer (§7.7), unless the user is already somewhere; after a target choice the old view's
  // control is gone, so focus lands back in the textarea.
  const hasComposerTarget = target.kind === 'direct' || routed || noTarget
  useEffect(() => {
    if (hasComposerTarget) focusIfIdle(composer.current)
  }, [hasComposerTarget])

  const toTarget = (c: { kind: 'routed' } | { kind: 'agent'; id: string }) =>
    navigate({
      to: '/chat',
      search: { ...carry, ...(c.kind === 'routed' ? { auto: 1 } : { agent: c.id }) },
      replace: true,
    })

  /** A user's choice (picker or target list): the text on screen follows it, and it is remembered (C2). */
  const choose = (c: TargetChoice) => {
    const to =
      c.kind === 'routed'
        ? newChatDraftKey({ routed: true })
        : newChatDraftKey({ agentId: c.agent.id })
    carryDraft(userId, draftKeyOf(agent), to)
    rememberTarget(
      userId,
      c.kind === 'routed' ? { kind: 'orchestrator' } : { kind: 'agent', id: c.agent.id },
    )
    void toTarget(c.kind === 'routed' ? c : { kind: 'agent', id: c.agent.id })
  }

  // The default target (C2, UC-A): the remembered one, else the only running agent; never once the
  // user has typed. A preselect replaces the URL and moves no draft (E16).
  const remembered = useMemo(
    () => (noTarget ? readRememberedTarget(userId) : null),
    [noTarget, userId],
  )
  const pre = noTarget
    ? preselectTarget({ remembered, agents: dir.data, typed: !!draft.trim() })
    : null
  const preKey = pre ? (pre.kind === 'routed' ? 'routed' : pre.id) : ''
  useEffect(() => {
    if (pre) void toTarget(pre)
  }, [preKey]) // eslint-disable-line react-hooks/exhaustive-deps

  // A create failure gives the text back (§6.1 step 4). The registry already put it in the
  // stored draft (onCreateFailed); this syncs the box when the view is still open.
  const failedCreate =
    (live?.phase === 'error' || live?.phase === 'not_started') && live.error?.phase === 'create'
  useEffect(() => {
    if (failedCreate && live)
      setDraftRaw(
        readDraft(
          userId,
          newChatDraftKey(live.agentId ? { agentId: live.agentId } : { routed: true }),
        ),
      )
  }, [failedCreate]) // eslint-disable-line react-hooks/exhaustive-deps

  /** Resolves true once the turn started; false when it was refused before anything was sent. */
  const send = async (text: string, retryId?: string): Promise<boolean> => {
    if (!agent && !routed) return false
    setSendError(null)
    const draftKey = draftKeyOf(agent)
    // The URL this send came from: create may take 20 s, and the user may have moved on.
    const sentFrom = router.state.location.href
    const onCreated = (row: ChatSessionRow) => {
      // A successful first send remembers the target (C2).
      rememberTarget(userId, agent ? { kind: 'agent', id: agent.id } : { kind: 'orchestrator' })
      // The create response carries no updated_at or agent_name; keep the list row readable.
      upsertSessionRow(client, {
        ...row,
        updated_at: row.updated_at ?? row.created_at,
        agent_name: row.agent_name ?? agent?.name ?? null,
      })
      // A history fetch made while the chat didn't exist yet (placeholder row clicked) got a 404.
      void client.invalidateQueries({ queryKey: chatKeys.history(row.session_id) })
      // Only move to the chat if this view is still the one the user sent from (EN8). The URL
      // alone isn't enough: leaving and coming back to the same URL makes a new view.
      if (mounted.current && router.state.location.href === sentFrom)
        void navigate({
          to: '/chat/$sessionId',
          params: { sessionId: row.session_id },
          search: carry,
          replace: true,
        })
    }
    // Runs from the registry, so it works even if this view is gone by then.
    const onCreateFailed = (id: string, failedText: string) => {
      removeSessionRow(client, id)
      if (!readDraft(userId, draftKey)) writeDraft(userId, draftKey, failedText)
    }
    // The placeholder row goes in before create starts, so a fast failure can always remove it.
    const onStart = (id: string) => {
      setPendingId(id)
      const at = new Date().toISOString()
      upsertSessionRow(client, {
        session_id: id,
        agent_id: agent?.id ?? null,
        agent_url: agent ? undefined : null,
        title: text.slice(0, 60),
        created_at: at,
        updated_at: at,
        agent_name: agent?.name ?? null,
      })
    }
    const common = {
      sessionId: retryId,
      create: !!retryId,
      text,
      onCreated,
      onCreateFailed,
      onStart,
    }
    const input: SendInput = agent
      ? { ...common, agentId: agent.id }
      : { ...common, chatMode: 'routed' }
    try {
      await registry.send(input)
      return true
    } catch (err) {
      // Give the text back, unless the user has typed something newer since (Try again).
      if (!readDraft(userId, draftKey).trim()) setDraftRaw(text)
      setSendError(err)
      return false
    }
  }
  const onSend = () => {
    const text = draft.trim()
    dropUndo(userId)
    setUndo(null)
    setDraftRaw('')
    void send(text)
  }
  const onExample = (text: string) => {
    if (draft.trim() && draft !== text) setReplaceWith(text)
    else {
      setDraft(text)
      composer.current?.focus()
    }
  }
  const onUndo = () => {
    const back = undoCarry(userId)
    setUndo(null)
    if (back !== null) setDraftRaw(back)
    composer.current?.focus()
  }

  const identity = agent
    ? targetIdentity({ kind: 'direct', agent })
    : routed
      ? targetIdentity({ kind: 'routed' })
      : null
  const directory = {
    isPending: dir.isPending,
    isError: dir.isError && !dir.data,
    retry: () => void dir.refetch(),
  }
  const running = chooserAgents(dir.data)
  // "Choose another agent" opens the target list under the chips, per target (user review, 2026-09-28): with a
  // target chosen the chips replace the list, and the picker was the only way to switch.
  const targetKey = agent?.id ?? (routed ? 'routed' : 'none')
  const [browsingFor, setBrowsingFor] = useState<string | null>(null)
  const rows = sessionRows(list.data)
  const identityOf = (r: ChatSessionRow) => chatIdentity(r, dir.byId, username)
  const recent = agent
    ? rows.filter((r) => r.agent_id === agent.id)
    : routed
      ? rows.filter((r) => chatKind(r) === 'routed')
      : rows.filter((r) => chatKind(r) !== 'recorded')
  const turn: DisplayTurn<LiveTurn> | null = live
    ? { key: 'pending', user: null, replies: [], requests: [], live }
    : null
  const handlers: TurnHandlers = {
    editMessage: (text) => {
      setDraft(text)
      composer.current?.focus()
    },
    viewTrace: () => undefined,
    refresh: () => undefined,
    runAgain: () => undefined,
    saveAgain: () => undefined,
    discard: () => undefined,
    // Clear the box only if it still holds the retried text; anything newer is the user's.
    tryAgain: () =>
      live &&
      void send(live.userText, live.sessionId).then((ok) => {
        if (ok && readDraft(userId, draftKeyOf(agent)).trim() === live.userText) setDraftRaw('')
      }),
    requests: {
      resolve: () => Promise.reject(new Error('no request yet')),
      cancel: () => Promise.reject(new Error('no request yet')),
      onDone: () => undefined,
    },
  }

  // The composer's lock: no target opens the picker (DS3); a stopped agent says why (E16); a turn in flight locks.
  let mode: ComposerMode = { kind: 'ready' }
  if (noTarget)
    mode = { kind: 'locked', reason: copy.needTarget, blocked: () => setPickerOpen(true) }
  else if (target.kind === 'loading' || target.kind === 'invalid') mode = { kind: 'locked' }
  else if (agent && agent.status !== 'running') {
    const reason = copy.stoppedTarget(agentLabel(agent))
    mode = { kind: 'locked', reason, blocked: () => announce(reason) }
  }
  if (live && isLivePhase(live.phase)) mode = { kind: 'locked' }

  const centred =
    !turn && (noTarget || routed || target.kind === 'direct' || target.kind === 'loading')
  const composerEl = (inline: boolean) => (
    <Composer
      ref={composer}
      inline={inline}
      agentName={agent ? agentLabel(agent) : undefined}
      label={routed ? copy.askOrchestrator : undefined}
      placeholder={
        routed
          ? copy.askOrchestrator
          : agent
            ? copy.composerLabel(agentLabel(agent))
            : copy.composerPlaceholder
      }
      note={routed && live && isLivePhase(live.phase) ? copy.routedHint : undefined}
      targetSlot={
        centred && target.kind !== 'loading' ? (
          <TargetPicker
            current={identity}
            agents={dir.data}
            directory={directory}
            open={pickerOpen}
            onOpenChange={setPickerOpen}
            onChoose={choose}
          />
        ) : undefined
      }
      value={draft}
      onChange={setDraft}
      onSend={onSend}
      onStop={() => undefined}
      onGoToRequest={() => undefined}
      onNewChat={() => undefined}
      mode={mode}
    />
  )

  let body: ReactNode = null
  if (turn && live) {
    body = (
      <div className={cn(CHAT_COLUMN, 'py-6')}>
        <TurnView
          turn={turn}
          latest
          agentName={agent ? agentLabel(agent) : copy.theOrchestrator}
          agentId={agent?.id ?? null}
          sessionId={live.sessionId}
          status={null}
          handlers={handlers}
          routed={routed ? { agents: dir.data, endFor: () => undefined } : undefined}
        />
      </div>
    )
  } else if (target.kind === 'invalid') {
    body = (
      <div className={CHAT_COLUMN}>
        <TargetBanner
          action={
            <Link
              to="/agents"
              search={{}}
              ref={focusIfIdle}
              className="inline-flex items-center text-primary-text underline-offset-4 hover:underline pointer-coarse:min-h-11"
            >
              {copy.allAgents}
            </Link>
          }
        >
          {copy.targetInvalid(target.value)}
        </TargetBanner>
      </div>
    )
  } else if (target.kind === 'ambiguous') {
    body = (
      <AgentChooser
        agents={target.agents}
        title={copy.targetAmbiguous(target.value)}
        hint=""
        carry={carry}
      />
    )
  } else if (centred) {
    const examples = agent
      ? agentExamples(agent)
      : routed
        ? orchestratorExamples(dir.data).map((e) => e.text)
        : []
    body = (
      <div className={cn(CHAT_COLUMN, 'flex flex-col gap-6 pb-10')} data-testid="new-chat">
        {/* The hero and composer sit around 40% of the pane; the block top-aligns once it outgrows it (DS8). */}
        <div aria-hidden className="h-[clamp(1.5rem,calc(30dvh-10rem),14rem)] shrink-0" />
        {target.kind === 'loading' ? (
          // The hero's own size, so the composer below stays put (and mounted) while targets load.
          <PageLoader label={copy.loading} inline className="min-h-24 flex-none" />
        ) : (
          <NewChatHero target={identity} description={agent?.description} />
        )}
        {agent && agent.status !== 'running' ? (
          <NotRunningBanner
            agentId={agent.id}
            name={agentLabel(agent)}
            refetch={() => void dir.refetch()}
          />
        ) : null}
        <div>
          {composerEl(true)}
          <SendError error={sendError} inline />
          {undo ? (
            <p className="mt-1.5 px-1 text-xs text-muted-foreground" data-testid="undo-draft">
              {copy.replacedDraft} ·{' '}
              <Button
                type="button"
                variant="link"
                className="h-auto p-0 text-xs font-normal pointer-coarse:min-h-11"
                onClick={onUndo}
              >
                {copy.undo}
              </Button>
            </p>
          ) : null}
        </div>
        {target.kind === 'loading' ? null : (
          <>
            <ExampleChips examples={examples} onExample={onExample} />
            {/* With no chips (or once asked), the list shows: the other agents, plus the Orchestrator unless it's the target. */}
            {examples.length ? (
              <div className="flex justify-center">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="text-muted-foreground pointer-coarse:min-h-11"
                  aria-expanded={browsingFor === targetKey}
                  aria-controls={browsingFor === targetKey ? 'choose-another' : undefined}
                  onClick={() => setBrowsingFor(browsingFor === targetKey ? null : targetKey)}
                >
                  {copy.chooseAnother}
                  <ChevronDown
                    aria-hidden
                    className={cn(
                      'transition-transform motion-reduce:transition-none',
                      browsingFor === targetKey && 'rotate-180',
                    )}
                  />
                </Button>
              </div>
            ) : null}
            {!examples.length || browsingFor === targetKey ? (
              <TargetList
                id="choose-another"
                agents={agent ? running.filter((a) => a.id !== agent.id) : running}
                withOrchestrator={!routed}
                directory={directory}
                onChoose={choose}
              />
            ) : null}
          </>
        )}
        <RecentChats rows={recent} identityOf={identityOf} carry={carry} sameTarget={!noTarget} />
      </div>
    )
  }

  return (
    <>
      <title>{`${titlePrefix}${agent ? copy.askAgent(agentLabel(agent)) : routed ? copy.askOrchestrator : copy.chatsNav} · OpenRuntime`}</title>
      <header className="flex h-12 shrink-0 items-center gap-2 border-b border-border px-3">
        {railButton}
        <h2 className="truncate text-sm font-medium">{copy.newChat}</h2>
      </header>
      <UnknownScenarioBanner mock={search.mock} />
      <div className="min-h-0 flex-1 overflow-y-auto">{body}</div>
      {search.debug ? <DebugFrames live={live} /> : null}
      {centred ? null : <SendError error={sendError} />}
      {!centred && (turn || target.kind === 'invalid') ? composerEl(false) : null}
      <StatusAnnouncer
        live={live}
        agentName={agent ? agentLabel(agent) : routed ? copy.theOrchestrator : ''}
        error={sendErrorText(sendError)}
      />
      <ConfirmDialog
        open={replaceWith !== null}
        onOpenChange={(o) => !o && setReplaceWith(null)}
        title={copy.replaceDraft}
        body={copy.replaceDraftBody}
        primary={{
          label: copy.replace,
          onClick: () => {
            setDraft(replaceWith ?? '')
            setReplaceWith(null)
          },
        }}
        onCloseAutoFocus={(e) => {
          e.preventDefault()
          composer.current?.focus()
        }}
      />
    </>
  )
}

export const NewChatViewMemo = memo(NewChatView)
