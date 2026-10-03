/**
 * The header's Setup guide: the onboarding guide where the server has it, else the first-run steps in a sheet, so they stay one click away after the first agent (the
 * first-run card shows them inline only while there are none). Deploy an agent first, the CLI steps as the alternative.
 */
import { BookOpen } from 'lucide-react'
import { Button } from '@/components/ui/button'
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from '@/components/ui/sheet'
import { FirstRunSteps } from '@/features/agents/components/bits'
import { useGuide } from '@/features/onboarding/api'
import { SetupGuideButton } from '@/features/onboarding/GuideCard'
import { DeployAgentButton } from '@/features/deploy/components/DeployAgentButton'
import { copy as deployCopy } from '@/features/deploy/copy'
import { copy } from '../copy'
import { TOUCH } from './Card'

export function SetupGuide() {
  // A server with the onboarding endpoint reopens the guide itself (spec §4); an older one keeps this sheet.
  const { absent } = useGuide()
  if (!absent) return <SetupGuideButton className={TOUCH} />
  return (
    <Sheet>
      <SheetTrigger asChild>
        <Button variant="outline" size="sm" className={TOUCH}>
          <BookOpen aria-hidden /> {copy.setup.button}
        </Button>
      </SheetTrigger>
      <SheetContent className="gap-0 sm:max-w-md">
        <SheetHeader>
          <SheetTitle>{copy.setup.title}</SheetTitle>
          <SheetDescription>{copy.setup.intro}</SheetDescription>
        </SheetHeader>
        <div className="flex flex-col gap-2 overflow-y-auto px-4 pb-4">
          <DeployAgentButton size="default" className={`self-start ${TOUCH}`} />
          <p className="mt-3 text-xs text-muted-foreground">{deployCopy.entry.orCli}</p>
          <FirstRunSteps />
        </div>
      </SheetContent>
    </Sheet>
  )
}
