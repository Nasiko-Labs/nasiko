"use client";

import { useState } from "react";
import { Button } from "@nasiko/ui/primitives";
import { MetricGroup } from "@nasiko/ui/composites";

export default function KpiFixture() {
  const [v, setV] = useState(1204.5);
  return (
    <div className="flex flex-col gap-3">
      <MetricGroup
        metrics={[
          { id: "spend", label: "Spend", value: { kind: "value", text: `$${v.toFixed(2)}` } },
        ]}
        columns={2}
      />
      <Button
        size="sm"
        className="self-start"
        data-testid="change"
        onClick={() => {
          setV((x) => x + 13.25);
        }}
      >
        New value
      </Button>
    </div>
  );
}
