/**
 * The product data layer for authored screens (`@nasiko/data-access/product`): the lab's REST
 * client, its error meanings, auth and health calls, the QueryClient factory and scoped query
 * keys. React lives only in `./react` (re-exported here as a "use client" module).
 */
export {
  fetchMe,
  lockedFor,
  login,
  logout,
  ME_FEATURE,
  meQueryOptions,
  safeRedirect,
  type CallOptions,
  type LoginResult,
  type Me,
} from "./auth";
export {
  apiData,
  apiFetch,
  configureProductApi,
  isAbortError,
  isRetryable,
  isTimeoutError,
  ProductApiError,
  safeJson,
  withQuery,
  type ApiInit,
  type Checked,
  type ProductApiConfig,
  type WireSubset,
} from "./client";
export { errorCode, isDeadSession401, isEndpointAbsent, isUnauthorized } from "./detect";
export {
  fetchHealth,
  HEALTH_FEATURE,
  HEALTH_INTERVAL_MS,
  healthQueryOptions,
  SIDEBAR_HEALTH_INTERVAL_MS,
  type HealthQueryOptions,
} from "./health";
export { isProductKey, productKey, productPrefix, type ProductKey } from "./keys";
export {
  createProductQueryClient,
  onCacheReset,
  type ProductQueryClientOptions,
} from "./query-client";
export {
  ProductQueryProvider,
  useProductKey,
  useProductScope,
  type ProductQueryProviderProps,
} from "./react";
export { keepPreviousData } from "@tanstack/query-core";
export * from "./sse";
