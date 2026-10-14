// Ported from nasiko-ui-lab@1b99bdda common/src/features/chat/components/TargetPicker.tsx. Local edits: `PopoverTrigger
// asChild` is `render={<Button …/>}` (Base UI useRender); the popover's `onCloseAutoFocus` is Base UI's `finalFocus`:
// the chip, where Radix always returned focus (Base UI goes back to the element focused before opening when the
// composer opened it), or false after a choice (the page moves focus); Prettier formatting.
/**
 * Where a new chat sends (v1c §5.4): a combobox chip at the composer's bottom left, labelled "Send to".
 * The Orchestrator first, a divider, running agents by name, then stopped agents with a muted status
 * word (full-contrast names, still selectable, DS13). Harnesses never appear. Esc returns focus to the
 * chip; a choice returns it to the textarea (the page moves it).
 */
import { ChevronDown, RotateCw, Route } from "lucide-react";
import { useRef, useState } from "react";
import {
  Button,
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
  Popover,
  PopoverContent,
  PopoverTrigger,
  Skeleton,
} from "@nasiko/ui/primitives";
import type { Agent } from "../../agents/types";
import { cn } from "@nasiko/ui/cn";
import { copy } from "../copy";
import { agentLabel, statusLabel } from "../format";
import type { ChatIdentity } from "../identity";
import { pickerAgents } from "../target";
import { IdentityIcon, StatusDot } from "./ChatIdentity";

export type TargetChoice = { kind: "routed" } | { kind: "agent"; agent: Agent };

export function TargetPicker({
  current,
  agents,
  directory,
  open,
  onOpenChange,
  onChoose,
}: {
  current: ChatIdentity | null;
  agents: readonly Agent[] | undefined;
  directory: { isPending: boolean; isError: boolean; retry: () => void };
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onChoose: (choice: TargetChoice) => void;
}) {
  const [query, setQuery] = useState("");
  const chose = useRef(false);
  const chip = useRef<HTMLButtonElement>(null);
  const { running, stopped } = pickerAgents(agents);
  const choose = (c: TargetChoice) => {
    chose.current = true;
    onOpenChange(false);
    onChoose(c);
  };
  const label = current?.name ?? copy.chooseTarget;
  return (
    <Popover
      open={open}
      onOpenChange={(o) => {
        if (o) chose.current = false;
        setQuery("");
        onOpenChange(o);
      }}
    >
      <PopoverTrigger
        render={
          <Button
            ref={chip}
            type="button"
            size="sm"
            variant="outline"
            aria-label={`${copy.sendTo}: ${label}`}
            title={label}
            className="h-8 max-w-[min(20rem,75%)] min-w-0 gap-1.5 rounded-full px-2.5 font-normal pointer-coarse:min-h-11"
            data-testid="target-chip"
          />
        }
      >
        {current?.kind === "orchestrator" ? (
          <Route aria-hidden className="size-3.5 text-primary-text" />
        ) : current?.kind === "agent" && current.status ? (
          <StatusDot running={current.status === "running"} />
        ) : null}
        <span className="truncate">{label}</span>
        <ChevronDown aria-hidden className="size-3.5 shrink-0 text-muted-foreground" />
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className="w-80 p-0"
        finalFocus={() => (chose.current ? false : (chip.current ?? true))}
      >
        <Command label={copy.sendTo}>
          <CommandInput
            aria-label={copy.sendTo}
            placeholder={copy.sendTo}
            value={query}
            onValueChange={setQuery}
          />
          <CommandList>
            <CommandEmpty>{copy.noAgentsMatch(query.trim())}</CommandEmpty>
            <CommandGroup>
              <CommandItem
                value={`${copy.orchestratorName} orchestrator`}
                onSelect={() => {
                  choose({ kind: "routed" });
                }}
                className="pointer-coarse:min-h-11"
              >
                <IdentityIcon kind="orchestrator" name={copy.orchestratorName} size={20} />
                <span className="flex min-w-0 flex-col">
                  <span className="truncate">{copy.orchestratorName}</span>
                  <span className="truncate text-xs text-muted-foreground">
                    {copy.identityOrchestratorSubline}
                  </span>
                </span>
              </CommandItem>
            </CommandGroup>
            <CommandSeparator />
            {directory.isPending ? (
              <div className="space-y-1 p-2" aria-busy>
                <span className="sr-only">{copy.loading}</span>
                {[0, 1, 2].map((i) => (
                  <Skeleton key={i} className="h-8 motion-reduce:animate-none" />
                ))}
              </div>
            ) : directory.isError && !agents ? (
              <div className="flex items-center gap-2 p-3 text-sm">
                {copy.agentsFailed}{" "}
                <Button
                  size="xs"
                  variant="outline"
                  className="pointer-coarse:min-h-11"
                  onClick={directory.retry}
                >
                  <RotateCw aria-hidden /> {copy.retry}
                </Button>
              </div>
            ) : (
              <CommandGroup>
                {[...running, ...stopped].map((a) => (
                  <CommandItem
                    key={a.id}
                    value={`${agentLabel(a)} ${a.name} ${a.id}`}
                    onSelect={() => {
                      choose({ kind: "agent", agent: a });
                    }}
                    className="pointer-coarse:min-h-11"
                    title={agentLabel(a)}
                  >
                    <IdentityIcon kind="agent" name={agentLabel(a)} size={20} />
                    <span className="min-w-0 flex-1 truncate">{agentLabel(a)}</span>
                    <span
                      className={cn(
                        "flex shrink-0 items-center gap-1.5 text-xs",
                        a.status === "running"
                          ? "text-muted-foreground"
                          : "text-muted-foreground italic",
                      )}
                    >
                      <StatusDot running={a.status === "running"} />
                      <span aria-hidden>{statusLabel(a.status)}</span>
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
