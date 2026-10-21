"use client";

import { useQuery } from "@tanstack/react-query";
import { healthQueryOptions, useProductScope } from "@nasiko/data-access/product";
import { PageHeader } from "@nasiko/ui/composites";
import { Card } from "@nasiko/ui/primitives";

export function StatusRoute() {
  const health = useQuery(healthQueryOptions(useProductScope()));
  const state = health.isPending ? "checking" : health.isError ? "unreachable" : "connected";
  return (
    <div className="flex max-w-2xl flex-col gap-4">
      <PageHeader title="Backend status" />
      <Card data-tour="status-backend" className="gap-0 p-4 text-sm">
        <div>
          Nasiko API connection: <strong data-testid="backend-state">{state}</strong>
        </div>
        {health.isError ? (
          <p className="mt-2 text-muted-foreground">
            Start it from <code>nasiko-cloud-rs</code> with <code>just run-stack</code>, or point{" "}
            <code>NASIKO_API_UPSTREAM</code> at it.
          </p>
        ) : null}
      </Card>
    </div>
  );
}
