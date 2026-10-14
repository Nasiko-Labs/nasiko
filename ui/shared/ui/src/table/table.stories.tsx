import type { Meta, StoryObj } from "@storybook/react-vite";
import { useEffect, useState } from "react";
import type { Page } from "@nasiko/data-access";
import { DataTable, type ColumnDef, type SortingState } from "./index";

type Row = { id: string; agent: string; model: string; requests: number; spend: string };
const ALL: Row[] = Array.from({ length: 53 }, (_, i) => ({
  id: `r${i}`,
  agent: ["alpha", "beta", "gamma"][i % 3] ?? "alpha",
  model: ["gpt-x", "claude-y", "local-z"][i % 3] ?? "gpt-x",
  requests: (i * 37) % 500,
  spend: `$${((i * 13.7) % 300).toFixed(2)}`,
}));
const columns: ColumnDef<Row>[] = [
  { accessorKey: "agent", header: "Agent" },
  { accessorKey: "model", header: "Model" },
  {
    accessorKey: "requests",
    header: "Requests",
    cell: (c) => <span className="tabular-nums">{c.getValue<number>()}</span>,
  },
  {
    accessorKey: "spend",
    header: "Spend",
    cell: (c) => <span className="tabular-nums">{c.getValue<string>()}</span>,
    enableSorting: false,
  },
];

/** A synthetic server: pages, sorts and totals happen *here*, never in the table. */
function serverPage(
  offset: number,
  limit: number,
  sorting: SortingState,
  knownTotal: boolean,
): Page<Row> {
  const s = sorting[0];
  const rows = [...ALL];
  if (s)
    rows.sort(
      (a, b) =>
        (String(a[s.id as keyof Row]) > String(b[s.id as keyof Row]) ? 1 : -1) * (s.desc ? -1 : 1),
    );
  return {
    rows: rows.slice(offset, offset + limit),
    offset,
    limit,
    total: knownTotal ? ALL.length : "unknown",
  };
}

const meta = { title: "Table/DataTable" } satisfies Meta;
export default meta;

export const ServerPaginated: StoryObj = {
  render: function Story() {
    const [sorting, setSorting] = useState<SortingState>([]);
    const [offset, setOffset] = useState(0);
    const [page, setPage] = useState<Page<Row> | null>(null);
    const [loading, setLoading] = useState(true);
    useEffect(() => {
      setLoading(true);
      const t = setTimeout(() => {
        setPage(serverPage(offset, 10, sorting, true));
        setLoading(false);
      }, 400);
      return () => {
        clearTimeout(t);
      };
    }, [offset, sorting]);
    return (
      <DataTable
        caption="Agent usage"
        columns={columns}
        page={page}
        loading={loading}
        stale={loading && page !== null}
        sorting={sorting}
        onSortChange={(s) => {
          setSorting(s);
          setOffset(0);
        }}
        onPageChange={setOffset}
      />
    );
  },
};

export const UnknownTotal: StoryObj = {
  render: () => (
    <DataTable
      caption="Agent usage"
      columns={columns}
      page={serverPage(10, 10, [], false)}
      onPageChange={() => undefined}
    />
  ),
};

export const EmptyAndError: StoryObj = {
  render: () => (
    <div className="flex flex-col gap-6">
      <DataTable
        caption="Empty"
        columns={columns}
        page={{ rows: [], offset: 0, limit: 10, total: 0 }}
        emptyMessage="No usage in this range"
      />
      <DataTable
        caption="Error"
        columns={columns}
        page={null}
        error="Usage service unavailable (request req_01HZX)"
      />
      <DataTable caption="Loading" columns={columns} page={null} loading />
    </div>
  ),
};
