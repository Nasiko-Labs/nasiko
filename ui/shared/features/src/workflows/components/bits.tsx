// Ported from nasiko-ui-lab@1b99bdda common/src/features/workflows/components/bits.tsx. Local edits: none beyond
// imports and Prettier; `SectionNav` is the app-core binding (port links, the open section's top-bar level; legacy
// ui/ on development, e8379db5a); `CreateWorkflowLink` (c721a11a8) is a `ButtonLink`.
import { Badge } from "@nasiko/ui/primitives";
import { SectionNav as SharedSectionNav } from "@nasiko/app-core/shell/SectionNav";
import { Plus } from "lucide-react";
import { ButtonLink } from "../../agents/components/ButtonLink";
import { copy } from "../copy";
import type { Tone } from "../logic";

const VARIANT = {
  success: "success",
  error: "destructive",
  warning: "warning",
  info: "info",
  neutral: "muted",
} as const satisfies Record<Tone, string>;

/** Text plus colour, never a colour alone. */
export function ToneBadge({ tone, children }: { tone: Tone; children: string }) {
  return (
    <Badge variant={VARIANT[tone]} data-tone={tone}>
      {children}
    </Badge>
  );
}

/** A quiet fact chip (steps, runs, tokens, durations). */
export function Chip({ children }: { children: string }) {
  return (
    <Badge variant="outline" className="font-normal text-muted-foreground">
      {children}
    </Badge>
  );
}

const SECTIONS = [
  { to: "/workflows", label: copy.nav.deployed },
  { to: "/workflows/drafts", label: copy.nav.drafts },
  { to: "/workflows/runs", label: copy.nav.runs },
] as const;

/** The module's one action, in the top bar on every section. */
export function CreateWorkflowLink() {
  return (
    <ButtonLink size="sm" className="pointer-coarse:min-h-11" href="/workflows/new">
      <Plus aria-hidden /> {copy.create}
    </ButtonLink>
  );
}

/** Deployed · Drafts · Runs: the three lists are one module (the React nav's three items). */
export function SectionNav({ current }: { current: (typeof SECTIONS)[number]["to"] }) {
  return <SharedSectionNav label={copy.nav.label} sections={SECTIONS} current={current} />;
}
