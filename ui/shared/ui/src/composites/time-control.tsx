// Ported from nasiko-ui-lab@1b99bdda common/src/components/shared/time-control.tsx. Local edits:
// - `PRESETS`/`Preset` are declared here (the lab's `lib/search.ts` is a zod + router search module; only this constant is
//   needed) and exported;
// - Base UI's ToggleGroup is value-array based with no `type="single"` (single is its default): `value={[preset]}`,
//   `onValueChange={(v) => pick(v[0] ?? "")}` — the lab's `pick` already ignores the empty deselect value;
// - the Select's `onValueChange` is Base UI's `(value, eventDetails)`; `pick` reads the first argument;
// - the Select gets `items={LABELS}` so its Value shows "Custom", not the raw "custom" (fidelity compare,
//   tokenops-custom--narrow: Base UI's Value reads labels from `items`, Radix read the rendered item);
// - imports mapped to ../primitives/*; optional props typed `| undefined` (exactOptionalPropertyTypes).
"use client";

/**
 * Single time control (plan: replaces the old range-vs-month dual mode; A1b collapses
 * it to a select on narrow screens).
 */
import { Input } from "../primitives/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../primitives/select";
import { ToggleGroup, ToggleGroupItem } from "../primitives/toggle-group";

export const PRESETS = ["24h", "7d", "30d", "mtd", "last-month", "custom"] as const;
export type Preset = (typeof PRESETS)[number];

const LABELS: Record<Preset, string> = {
  "24h": "24h",
  "7d": "7d",
  "30d": "30d",
  mtd: "This month",
  "last-month": "Last month",
  custom: "Custom",
};

export function TimeControl({
  preset,
  invalid,
  from,
  to,
  today,
  onChange,
}: {
  /** The preset in the URL, not the resolved one: an invalid custom range keeps its inputs. */
  preset: Preset;
  /** The custom range can't be used (inverted, impossible or in the future); the page shows 30d meanwhile. */
  invalid?: boolean | undefined;
  from?: string | undefined;
  to?: string | undefined;
  /** UTC date, the latest selectable custom end. */
  today: string;
  onChange: (next: { preset: Preset; from?: string | undefined; to?: string | undefined }) => void;
}) {
  const pick = (p: string) => {
    if (!p) return;
    const next = p as Preset;
    if (next === "custom") onChange({ preset: "custom", from: from ?? today, to: to ?? today });
    else onChange({ preset: next });
  };
  return (
    <div className="flex flex-wrap items-center gap-2">
      <div className="hidden md:block">
        <ToggleGroup
          variant="outline"
          size="sm"
          value={[preset]}
          onValueChange={(v: unknown[]) => {
            pick(typeof v[0] === "string" ? v[0] : "");
          }}
          aria-label="Time window"
        >
          {PRESETS.map((p) => (
            <ToggleGroupItem key={p} value={p} className="px-2.5 text-xs">
              {LABELS[p]}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      </div>
      <div className="md:hidden">
        <Select
          value={preset}
          onValueChange={(v: string | null) => {
            pick(v ?? "");
          }}
          // Base UI's Value shows a label (not the raw value) only for items it knows: the lab's Radix Value read
          // it from the rendered item.
          items={LABELS}
        >
          <SelectTrigger size="sm" aria-label="Time window" className="w-36">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {PRESETS.map((p) => (
              <SelectItem key={p} value={p}>
                {LABELS[p]}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>
      {preset === "custom" ? (
        <div className="flex flex-wrap items-center gap-1 text-xs">
          <label className="sr-only" htmlFor="tw-from">
            From (UTC)
          </label>
          <Input
            id="tw-from"
            type="date"
            className="h-8 w-36"
            value={from ?? ""}
            max={to ?? today}
            onChange={(e) => {
              onChange({ preset: "custom", from: e.target.value, to });
            }}
          />
          <span aria-hidden>–</span>
          <label className="sr-only" htmlFor="tw-to">
            To (UTC)
          </label>
          <Input
            id="tw-to"
            type="date"
            className="h-8 w-36"
            value={to ?? ""}
            min={from}
            max={today}
            onChange={(e) => {
              onChange({ preset: "custom", from, to: e.target.value });
            }}
            aria-invalid={invalid || undefined}
            aria-describedby={invalid ? "tw-invalid" : undefined}
          />
          {invalid ? (
            <span id="tw-invalid" role="status" className="basis-full text-sm text-warning">
              Invalid range, showing the last 30 days
            </span>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
