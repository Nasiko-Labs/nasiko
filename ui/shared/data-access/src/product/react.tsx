"use client";

import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import { createContext, useContext, useMemo, type ReactNode } from "react";
import { productKey, type ProductKey } from "./keys";
import type { Scope } from "../canonical";

/**
 * The one React entry of the product data layer. Everything else under `product/` is plain
 * TypeScript so Node tests stay light; this module provides the QueryClient and the scope
 * (workspace id + principal epoch, ADR-0001/ADR-0004) that every product query key carries.
 */
const ScopeContext = createContext<Scope | null>(null);

export interface ProductQueryProviderProps {
  readonly scope: Scope;
  readonly queryClient: QueryClient;
  readonly children: ReactNode;
}

export function ProductQueryProvider({ scope, queryClient, children }: ProductQueryProviderProps) {
  // A new object per workspaceId/epoch pair only, so consumers memoising on the scope don't churn.
  const value = useMemo<Scope>(
    () => ({ workspaceId: scope.workspaceId, epoch: scope.epoch }),
    [scope.workspaceId, scope.epoch],
  );
  // A scope change remounts the subtree: no observer survives a workspace switch, so
  // `placeholderData: keepPreviousData` (which keeps the observer's last data, not the cache's)
  // can never show the old workspace's rows under the new scope's key.
  return (
    <QueryClientProvider client={queryClient}>
      <ScopeContext.Provider key={`${value.workspaceId}\u0000${String(value.epoch)}`} value={value}>
        {children}
      </ScopeContext.Provider>
    </QueryClientProvider>
  );
}

/** The current scope; throws outside a ProductQueryProvider (a key without a scope would leak across workspaces). */
export function useProductScope(): Scope {
  const scope = useContext(ScopeContext);
  if (!scope) throw new Error("useProductScope: no ProductQueryProvider above this component");
  return scope;
}

/** `productKey` for the current scope. */
export function useProductKey(feature: string, ...parts: readonly unknown[]): ProductKey {
  const scope = useProductScope();
  return productKey(scope, feature, ...parts);
}
