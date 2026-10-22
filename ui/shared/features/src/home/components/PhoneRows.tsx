// Ported from the legacy ui/ on development (3aac84f66) common/src/features/home/components/PhoneRows.tsx. Local edits:
// links are the navigation port's `Link` (`href`, `hrefFor` for a search); Prettier formatting.
/**
 * The home's rows below 640 px (plans/feat-agent-centric.md design "Responsive"): two lines each, name · kind · health,
 * then spend · budget use, so nothing scrolls sideways (QA ISSUE-002). Same rows, order and groups as the table; the
 * table's other columns (saved, last change) are one tap away on the agent's hub. Each list shows its first
 * `PHONE_ROWS` rows and a "Show N more" button (decision #50): phones never draw a whole large fleet at once.
 */
import Link from "@nasiko/app-core/navigation";
import { useState } from "react";
import { Badge } from "@nasiko/ui/primitives";
import { Button } from "@nasiko/ui/primitives";
import { budgetsShown } from "../api";
import { copy } from "../copy";
import { ownerLine, rowGroups } from "../rows";
import { PHONE_ROWS, TOUCH } from "../tuning";
import { BudgetUse, Quiet, SpendFigure } from "./RowCells";
import { GroupLabel, Health, type RowsProps } from "./RowsTable";

export function PhoneRows(p: RowsProps) {
  const groups = rowGroups(p.rows, p.grouped, p.ownerName);
  return (
    <section aria-label={copy.table.label} className="flex flex-col gap-3">
      {groups ? (
        groups.map((g) => (
          <section key={g.key} aria-label={g.label} className="flex flex-col gap-1">
            <h2 className="m-0 text-xs font-medium">
              <GroupLabel group={g} floor={p.unpriced} />
            </h2>
            <List {...p} rows={g.rows} />
          </section>
        ))
      ) : (
        <List {...p} />
      )}
    </section>
  );
}

function List({ rows, ...p }: RowsProps) {
  const budgets = budgetsShown(p.budgets.state);
  const [all, setAll] = useState(false);
  const shown = all ? rows : rows.slice(0, PHONE_ROWS);
  return (
    <>
      <ul className="m-0 flex list-none flex-col divide-y divide-border rounded-md border border-border p-0">
        {shown.map((r) => (
          <li
            key={r.id}
            data-testid="home-phone-row"
            className="flex min-w-0 flex-col gap-1 px-3 py-2"
          >
            <span className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
              {/* A long unbroken name wraps anywhere rather than push the page sideways. */}
              <Link
                href={`/agents/${encodeURIComponent(r.id)}`}
                className={`inline-flex min-w-0 items-center font-medium [overflow-wrap:anywhere] underline-offset-4 hover:underline ${TOUCH}`}
              >
                {r.displayName}
              </Link>
              {r.harness ? <Badge variant="outline">{copy.table.harness}</Badge> : null}
              {r.needs === null ? (
                <Quiet title={copy.phone.needsUnknownHint}>{copy.phone.needsUnknown}</Quiet>
              ) : r.needs > 0 ? (
                <Badge variant="outline">{copy.phone.needs(r.needs)}</Badge>
              ) : null}
              <Health row={r} pending={p.ratingsPending} />
            </span>
            <span className="flex flex-wrap items-baseline gap-x-3 text-sm tabular-nums">
              <span>
                <span className="text-muted-foreground">{copy.table.spend(p.days)} </span>
                <SpendFigure row={r} />
              </span>
              {budgets && r.budget ? (
                <span className="flex gap-1">
                  <span className="text-muted-foreground">{copy.table.budget}</span>
                  <BudgetUse row={r} state={p.budgets.state} />
                </span>
              ) : null}
            </span>
            {p.grouped ? null : (
              <span className="text-xs text-muted-foreground">{ownerLine(r, p.ownerName)}</span>
            )}
          </li>
        ))}
      </ul>
      {shown.length < rows.length ? (
        <Button
          variant="outline"
          size="sm"
          className={`self-start ${TOUCH}`}
          onClick={() => {
            setAll(true);
          }}
        >
          {copy.phone.more(rows.length - shown.length)}
        </Button>
      ) : null}
    </>
  );
}
