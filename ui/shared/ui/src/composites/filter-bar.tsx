"use client";

import { cn } from "../lib/cn";
import {
  apply,
  discardDraft,
  editDraft,
  isDirty,
  type GroupedFilterState,
} from "@nasiko/data-access";
import { Button } from "../primitives/button";
import { Input } from "../primitives/input";
import { Label } from "../primitives/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "../primitives/select";
import { StaleIndicator } from "./states";

/**
 * FilterBar bound to the data-access grouped-filter state machine: draft edits
 * change freely and *nothing is queried* until Apply (contract §13 grouped
 * filters, §8.3 "grouped Apply triggers once"). The bar owns no state; the
 * caller holds the `GroupedFilterState` and passes it back changed.
 *
 * Controls are typed (`select`, `text`, `preset-range`) — a search box and a
 * picker are different things (§8 "search/picker distinctions"), and date
 * ranges are presets here; a free calendar control is not admitted in Phase 3.
 */
export type FilterValues = Record<string, string>;

export type FilterField =
  | {
      kind: "select";
      key: string;
      label: string;
      options: readonly { value: string; label: string }[];
      placeholder?: string;
    }
  | { kind: "text"; key: string; label: string; placeholder?: string; maxLength?: number }
  | {
      kind: "preset-range";
      key: string;
      label: string;
      presets: readonly { value: string; label: string }[];
    };

export interface FilterBarProps<T extends FilterValues> {
  readonly fields: readonly FilterField[];
  readonly state: GroupedFilterState<T>;
  readonly onChange: (next: GroupedFilterState<T>) => void;
  /** A refetch for the applied version is in flight. */
  readonly loading?: boolean;
  readonly className?: string;
  readonly label?: string;
}

export function FilterBar<T extends FilterValues>({
  fields,
  state,
  onChange,
  loading = false,
  className,
  label = "Filters",
}: FilterBarProps<T>) {
  const dirty = isDirty(state);
  const set = (key: string, value: string) => {
    onChange(editDraft(state, { [key]: value } as Partial<T>));
  };
  return (
    <form
      role="search"
      aria-label={label}
      className={cn(
        "flex flex-wrap items-end gap-3 rounded-lg bg-card p-3 ring-1 ring-foreground/10",
        className,
      )}
      onSubmit={(e) => {
        e.preventDefault();
        onChange(apply(state));
      }}
    >
      {fields.map((f) => {
        const id = `filter-${f.key}`;
        const value = state.draft[f.key] ?? "";
        return (
          <div key={f.key} className="flex min-w-36 flex-col gap-1">
            <Label htmlFor={id}>{f.label}</Label>
            {f.kind === "text" ? (
              <Input
                id={id}
                value={value}
                placeholder={f.placeholder}
                maxLength={f.maxLength ?? 200}
                onChange={(e) => {
                  set(f.key, e.target.value);
                }}
              />
            ) : (
              <Select
                items={(f.kind === "select" ? f.options : f.presets).map((o) => ({
                  value: o.value,
                  label: o.label,
                }))}
                value={value}
                onValueChange={(v) => {
                  set(f.key, typeof v === "string" ? v : "");
                }}
              >
                <SelectTrigger id={id} className="w-full">
                  <SelectValue placeholder={f.kind === "select" ? f.placeholder : undefined} />
                </SelectTrigger>
                <SelectContent>
                  {(f.kind === "select" ? f.options : f.presets).map((o) => (
                    <SelectItem key={o.value} value={o.value}>
                      {o.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            )}
          </div>
        );
      })}
      <div className="ml-auto flex items-center gap-2">
        <StaleIndicator visible={loading} />
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={!dirty}
          onClick={() => {
            onChange(discardDraft(state));
          }}
        >
          Discard
        </Button>
        <Button type="submit" size="sm" disabled={!dirty} aria-describedby={`${label}-status`}>
          Apply
        </Button>
        <span id={`${label}-status`} className="sr-only" aria-live="polite">
          {dirty
            ? "Filters changed; apply to update results."
            : `Filters applied (version ${state.appliedVersion}).`}
        </span>
      </div>
    </form>
  );
}
