// Ported from nasiko-ui-lab@1b99bdda common/src/features/tokenops/catalog.query.test.ts. Local edits: the client
// and query-client factory come from `@nasiko/data-access/product` (`createProductQueryClient({ onUnauthorized,
// retry: false })`), `providersQuery` takes the mock scope; the pinned seed is `@/test/pinned-seed`; Prettier
// formatting.
/** The model catalog query dedupes the server's overlapping open pricing rows (catalog.ts), as the mock returns them. */
import { describe, expect, it } from "vitest";
import { apiFetch, createProductQueryClient } from "@nasiko/data-access/product";
import { setupPinnedSeed } from "@nasiko/testing/pinned-seed";
import { MOCK_SCOPE } from "./test-utils";
import { providersQuery } from "./api";
import type { ProviderCatalogEntry } from "./types";

setupPinnedSeed();

describe("providersQuery", () => {
  it("lists each model once per provider, keeping the newest pricing row", async () => {
    const raw = (await apiFetch<{ data: ProviderCatalogEntry[] }>("/api/llm-router/providers"))
      .data;
    const count = (groups: ProviderCatalogEntry[]) =>
      groups.flatMap((g) => g.models).filter((m) => m.model === "gemini-2.0-flash").length;
    expect(count(raw)).toBe(2);
    const groups = await createProductQueryClient({
      onUnauthorized: () => undefined,
      retry: false,
    }).query(providersQuery(MOCK_SCOPE));
    expect(count(groups)).toBe(1);
    for (const g of groups)
      expect(new Set(g.models.map((m) => m.model)).size).toBe(g.models.length);
    expect(groups.flatMap((g) => g.models).find((m) => m.model === "gemini-2.0-flash")?.notes).toBe(
      "gemini-2.0-flash",
    );
  });
});
