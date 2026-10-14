"use client";

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

/** Portalled overlays: they render into <body>, outside the provider's wrapper. */
export default function OverlayFixture() {
  return (
    <div className="flex gap-3">
      <Dialog>
        <DialogTrigger render={<Button size="sm" data-testid="open-dialog" />}>
          Open dialog
        </DialogTrigger>
        <DialogContent data-testid="dialog">
          <DialogHeader>
            <DialogTitle>Dialog</DialogTitle>
            <DialogDescription>Portalled content.</DialogDescription>
          </DialogHeader>
        </DialogContent>
      </Dialog>
      <Popover>
        <PopoverTrigger render={<Button size="sm" variant="outline" data-testid="open-popover" />}>
          Open popover
        </PopoverTrigger>
        <PopoverContent data-testid="popover">Popover content</PopoverContent>
      </Popover>
    </div>
  );
}
