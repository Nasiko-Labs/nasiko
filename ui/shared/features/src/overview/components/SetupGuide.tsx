// Ported from nasiko-ui-lab@1b99bdda common/src/features/overview/components/SetupGuide.tsx. Local edits: `"use
// client"`; `SheetTrigger asChild` + `Button` -> `render={<Button …/>}`; Prettier formatting.
"use client";

/**
 * The header's Setup guide: the onboarding guide where the server has it, else the first-run steps in a sheet, so they stay one click away after the first agent (the
 * first-run card shows them inline only while there are none). Deploy an agent first, the CLI steps as the alternative.
 */
import { BookOpen } from "lucide-react";
import { Button } from "@nasiko/ui/primitives";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
  SheetTrigger,
} from "@nasiko/ui/primitives";
import { FirstRunSteps } from "../../agents/components/bits";
import { useGuide } from "../../onboarding/api";
import { SetupGuideButton } from "../../onboarding/GuideCard";
import { DeployAgentButton } from "../../deploy/components/DeployAgentButton";
import { copy as deployCopy } from "../../deploy/copy";
import { copy } from "../copy";
import { TOUCH } from "./Card";

export function SetupGuide() {
  // A server with the onboarding endpoint reopens the guide itself (spec §4); an older one keeps this sheet.
  const { absent } = useGuide();
  if (!absent) return <SetupGuideButton className={TOUCH} />;
  return (
    <Sheet>
      <SheetTrigger render={<Button variant="outline" size="sm" className={TOUCH} />}>
        <BookOpen aria-hidden /> {copy.setup.button}
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
  );
}
