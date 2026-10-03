/**
 * The workflow's description, which doubles as the planner's prompt: Enter (outside an IME composition) or the button
 * asks for a plan. Once the steps on screen came from this text, asking again is off until it is edited.
 * `confirmReplace`: the steps were edited since, so replacing them asks first.
 */
import { Link } from '@tanstack/react-router'
import { Loader2, WandSparkles } from 'lucide-react'
import { useState } from 'react'
import { Alert, AlertDescription } from '@/components/ui/alert'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import {
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupTextarea,
} from '@/components/ui/input-group'
import { copy, reason } from '../copy'
import { planFailure } from '../logic'

/** Why the planner said no, and what the person can do instead. */
function PlanNotice({ error }: { error: unknown }) {
  const why = planFailure(error)
  if (why === 'no-agents')
    return (
      <>
        {copy.plan.noAgentsBefore}{' '}
        <Link to="/deploy" className="text-primary-text underline underline-offset-4">
          {copy.plan.noAgentsLink}
        </Link>{' '}
        {copy.plan.noAgentsAfter}
      </>
    )
  if (why === 'other') return copy.plan.other(reason(error))
  return copy.plan[why]
}

export function PlanComposer({
  value,
  onChange,
  onGenerate,
  generatedFrom,
  confirmReplace,
  busy,
  error,
  placeholder,
  disabled,
}: {
  value: string
  onChange: (value: string) => void
  onGenerate: (description: string) => void
  /** The description the on-screen steps were generated from (null: none). */
  generatedFrom: string | null
  confirmReplace: boolean
  busy: boolean
  error: unknown
  placeholder: string
  disabled?: boolean
}) {
  const [asking, setAsking] = useState<string | null>(null)
  const desc = value.trim()
  const canSubmit = !!desc && !busy && !disabled && desc !== generatedFrom
  const submit = () => {
    if (!canSubmit) return
    if (confirmReplace) setAsking(desc)
    else onGenerate(desc)
  }
  return (
    <div className="flex flex-col gap-3">
      <InputGroup>
        <InputGroupTextarea
          id="wf-desc"
          aria-label={copy.descLabel}
          placeholder={placeholder}
          value={value}
          disabled={disabled}
          rows={3}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault()
              submit()
            }
          }}
        />
        <InputGroupAddon align="block-end" className="justify-end">
          <InputGroupButton variant="default" size="sm" disabled={!canSubmit} onClick={submit}>
            {busy ? <Loader2 className="animate-spin" aria-hidden /> : <WandSparkles aria-hidden />}
            {generatedFrom === null ? copy.generate : copy.regenerate}
          </InputGroupButton>
        </InputGroupAddon>
      </InputGroup>
      {error && !busy ? (
        <Alert>
          <AlertDescription>
            <PlanNotice error={error} />
          </AlertDescription>
        </Alert>
      ) : null}
      {busy ? (
        <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 aria-hidden className="size-4 animate-spin" /> {copy.generating}
        </p>
      ) : null}
      <AlertDialog open={asking !== null} onOpenChange={(o) => !o && setAsking(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{copy.replaceTitle}</AlertDialogTitle>
            <AlertDialogDescription>{copy.replaceText}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{copy.keepSteps}</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (asking) onGenerate(asking)
                setAsking(null)
              }}
            >
              {copy.replaceSteps}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
