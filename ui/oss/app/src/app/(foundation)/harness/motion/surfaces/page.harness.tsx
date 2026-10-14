"use client";

import Link from "@nasiko/app-core/navigation";
import { useState } from "react";
import type { MotionPreference } from "@nasiko/contracts";
import { MotionProvider, useMotion } from "@nasiko/ui/motion";
import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@nasiko/ui/primitives";

/**
 * Two sibling surfaces with different policies, each opening its own dialog.
 * Surface A can be unmounted; the page links elsewhere so navigation away and
 * back is testable. Nothing here touches the application preference.
 */
function Surface({ id, preference }: { id: string; preference: MotionPreference }) {
  return (
    <MotionProvider preference={preference}>
      <SurfaceBody id={id} />
    </MotionProvider>
  );
}

function SurfaceBody({ id }: { id: string }) {
  const m = useMotion();
  return (
    <section
      data-testid={`surface-${id}`}
      data-mode={m.mode}
      className="flex flex-col gap-2 rounded-lg bg-card p-3 ring-1 ring-foreground/10"
    >
      <p className="text-xs text-muted-foreground">
        surface {id}: mode={m.mode} preference={m.preference}
      </p>
      <Dialog>
        <DialogTrigger render={<Button size="sm" data-testid={`open-${id}`} />}>
          Open dialog {id}
        </DialogTrigger>
        <DialogContent data-testid={`dialog-${id}`}>
          <DialogHeader>
            <DialogTitle>Dialog {id}</DialogTitle>
            <DialogDescription>Portalled by surface {id}.</DialogDescription>
          </DialogHeader>
        </DialogContent>
      </Dialog>
    </section>
  );
}

export default function SurfacesFixture() {
  const [showA, setShowA] = useState(true);
  return (
    <div className="flex flex-col gap-3">
      <div className="flex gap-2">
        <Button
          size="sm"
          variant="outline"
          data-testid="toggle-a"
          onClick={() => {
            setShowA((v) => !v);
          }}
        >
          {showA ? "Unmount" : "Mount"} surface A
        </Button>
        <Link href="/harness/motion/recipes" className="text-sm underline" data-testid="away">
          away
        </Link>
      </div>
      <div className="grid gap-3 md:grid-cols-2">
        {showA ? <Surface id="a" preference="off" /> : null}
        <Surface id="b" preference="system" />
      </div>
    </div>
  );
}
