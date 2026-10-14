// Ported from nasiko-ui-lab@1b99bdda common/src/features/deploy/api.ts (`recentUploadsQuery` only). Local edits: split
// out of ./api.ts, which re-exports it, so the background follower reads it without importing ./api.ts (the lab's
// api → uploads → follower → api import cycle fails the architecture check); the key is scoped,
// `productKey(scope, "builds", "uploads", limit)` (the lab's `['builds', 'uploads', limit]`); Prettier formatting.
import { queryOptions } from "@tanstack/react-query";
import type { Scope } from "@nasiko/data-access";
import { apiData, productKey } from "@nasiko/data-access/product";
import type { UploadStatus } from "./types";

/** The recent uploads the background follower (eng review R8) and M2 read; enveloped `{data, status_code, message}`. */
export function recentUploadsQuery(scope: Scope, limit: number) {
  return queryOptions({
    queryKey: productKey(scope, "builds", "uploads", limit),
    queryFn: ({ signal }) =>
      apiData<UploadStatus[]>(`/api/agents/uploads?limit=${limit}`, { signal }),
    retry: false,
    meta: { path: "/api/agents/uploads" },
  });
}
