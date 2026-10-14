"use client";

import { useState } from "react";
import { Button } from "@nasiko/ui/primitives";
import { Presence } from "@nasiko/ui/motion";

export default function PanelFixture() {
  const [open, setOpen] = useState(false);
  const [clicks, setClicks] = useState(0);
  return (
    <div className="flex flex-col gap-3">
      <Button
        size="sm"
        className="self-start"
        data-testid="toggle"
        onClick={() => {
          setOpen((o) => !o);
        }}
      >
        {open ? "Close" : "Open"}
      </Button>
      <Presence show={open} recipe="panel" from="end">
        <div data-testid="panel" className="w-64 rounded-lg bg-card p-4 ring-1 ring-foreground/10">
          <Button
            size="sm"
            variant="outline"
            data-testid="panel-action"
            onClick={() => {
              setClicks((c) => c + 1);
            }}
          >
            Action
          </Button>
        </div>
      </Presence>
      <span data-testid="clicks">{clicks}</span>
    </div>
  );
}
