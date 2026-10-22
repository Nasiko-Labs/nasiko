export { DataTable, type DataTableProps } from "./data-table";
export type { ColumnDef, SortingState } from "@tanstack/react-table";
// Ported from nasiko-ui-lab@1b99bdda: the lab's virtualised, client-sortable table (TanStack Table 8 here).
export {
  VirtualDataTable,
  type VirtualDataTableProps,
  type VirtualDataTableColumn,
  type VirtualDataTableGroups,
} from "./virtual-data-table";
