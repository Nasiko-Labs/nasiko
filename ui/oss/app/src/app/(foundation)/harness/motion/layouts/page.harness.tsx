"use client";

import type { CSSProperties } from "react";
import { MotionProvider, useMotion } from "@nasiko/ui/motion";
import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@nasiko/ui/primitives";

/**
 * Surfaces inside the three host layouts a Weave surface host is likely to be
 * mounted in: a scrolling panel, a transformed container (a containing block
 * for `position: fixed`) and a clipped container. Each surface has its own
 * policy (`off`) and opens a dialog and a popover; the test asserts the
 * overlays are fully visible and still governed by the surface's policy.
 */
const HOSTS = {
  scroll: {
    label: "overflow:auto, 160px tall, trigger below the fold",
    style: { overflow: "auto", height: 120 },
  },
  transform: {
    label: "transform: translateZ(0) (containing block for fixed descendants)",
    style: { transform: "translateZ(0)" },
  },
  clip: {
    label: "overflow:hidden, 44px tall",
    style: { overflow: "hidden", height: 44 },
  },
  filter: {
    label: "filter: blur(0) (containing block)",
    style: { filter: "blur(0px)" },
  },
  "transform-clip": {
    label: "transform + overflow:hidden, 44px tall (a fixed overlay would be clipped to the host)",
    style: { transform: "translateZ(0)", overflow: "hidden", height: 44 },
  },
} satisfies Record<string, { style: CSSProperties; label: string }>;

function Host({ id }: { id: keyof typeof HOSTS }) {
  const h = HOSTS[id];
  return (
    <div
      data-testid={`host-${id}`}
      style={h.style}
      className="rounded-lg bg-muted/40 p-3 ring-1 ring-foreground/10"
    >
      <p className="mb-2 text-xs text-muted-foreground">{h.label}</p>
      {id === "scroll" ? <div style={{ height: 220 }} aria-hidden /> : null}
      <MotionProvider preference="off">
        <SurfaceBody id={id} />
      </MotionProvider>
    </div>
  );
}

function SurfaceBody({ id }: { id: string }) {
  const m = useMotion();
  return (
    <section data-testid={`surface-${id}`} data-mode={m.mode} className="flex gap-2">
      <Dialog>
        <DialogTrigger render={<Button size="sm" data-testid={`open-dialog-${id}`} />}>
          Dialog
        </DialogTrigger>
        <DialogContent data-testid={`dialog-${id}`}>
          <DialogHeader>
            <DialogTitle>Dialog in {id}</DialogTitle>
            <DialogDescription>Portalled by the {id} surface.</DialogDescription>
          </DialogHeader>
          {/* Tall enough that a host-clipped overlay cannot hide the clipping. */}
          <div style={{ height: 240 }} data-testid={`dialog-body-${id}`} />
        </DialogContent>
      </Dialog>
      <Popover>
        <PopoverTrigger
          render={<Button size="sm" variant="outline" data-testid={`open-popover-${id}`} />}
        >
          Popover
        </PopoverTrigger>
        <PopoverContent data-testid={`popover-${id}`}>Popover in {id}</PopoverContent>
      </Popover>
    </section>
  );
}

export default function LayoutsFixture() {
  return (
    <div className="grid gap-4 md:grid-cols-2">
      <Host id="scroll" />
      <Host id="transform" />
      <Host id="clip" />
      <Host id="filter" />
      <Host id="transform-clip" />
    </div>
  );
}
