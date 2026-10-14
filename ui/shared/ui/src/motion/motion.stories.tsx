import type { Meta, StoryObj } from "@storybook/react-vite";
import { useState } from "react";
import { Button } from "../primitives/button";
import { Card, CardContent, CardHeader, CardTitle } from "../primitives/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "../primitives/dialog";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "../primitives/tabs";
import { MOTION_INVENTORY } from "./inventory";
import { ExpressiveRegion, RestoredSurface, useMotion } from "./provider";
import { Emphasis, Presence, Reveal, StaggerGroup } from "./recipes";

/**
 * Motion matrix stories (contract §8.3/§8.6). Use the toolbar globals
 * (Profile / Motion) to see each recipe under operational/expressive/none ×
 * system/reduced/off. Every story is also what `test:motion` drives.
 */
const meta = { title: "Motion/Matrix" } satisfies Meta;
export default meta;

function PolicyReadout() {
  const m = useMotion();
  return (
    <p className="text-xs text-muted-foreground" data-testid="policy">
      mode=<b>{m.mode}</b> profile=<b>{m.profile}</b> preference=<b>{m.preference}</b> os-reduced=
      <b>{String(m.osReduced)}</b>
    </p>
  );
}

export const Recipes: StoryObj = {
  render: function RecipesDemo() {
    const [key, setKey] = useState(0);
    const m = useMotion();
    return (
      <div className="flex flex-col gap-4">
        <PolicyReadout />
        <Button
          size="sm"
          className="self-start"
          onClick={() => {
            setKey((k) => k + 1);
          }}
        >
          Replay entrances
        </Button>
        <div key={key} className="grid gap-3 md:grid-cols-3">
          {(["reveal", "section", "panel", "showcase"] as const).map((r) => {
            const e = m.recipe(r);
            return (
              <Reveal key={r} recipe={r}>
                <Card size="sm">
                  <CardHeader>
                    <CardTitle>{r}</CardTitle>
                  </CardHeader>
                  <CardContent className="text-xs text-muted-foreground">
                    effective={e.effective} · {e.durationMs} ms · {e.distancePx} px
                    {e.opacityOnly ? " · opacity only" : ""}
                    {e.clampedBy ? ` · clamped: ${e.clampedBy}` : ""}
                  </CardContent>
                </Card>
              </Reveal>
            );
          })}
        </div>
      </div>
    );
  },
};

export const Stagger: StoryObj = {
  name: "Stagger (≤6 delayed, cap 150 ms)",
  render: function StaggerDemo() {
    const [key, setKey] = useState(0);
    return (
      <div className="flex flex-col gap-3">
        <PolicyReadout />
        <Button
          size="sm"
          className="self-start"
          onClick={() => {
            setKey((k) => k + 1);
          }}
        >
          Replay
        </Button>
        <StaggerGroup key={key} className="grid grid-cols-4 gap-2">
          {Array.from({ length: 10 }, (_, i) => (
            <div key={i} className="rounded bg-muted p-3 text-center text-sm">
              {i + 1}
            </div>
          ))}
        </StaggerGroup>
      </div>
    );
  },
};

export const Panel: StoryObj = {
  name: "Panel presence (rapid toggle, exit is inert)",
  render: function PanelDemo() {
    const [open, setOpen] = useState(false);
    return (
      <div className="flex flex-col gap-3">
        <PolicyReadout />
        <Button
          size="sm"
          className="self-start"
          data-testid="toggle"
          onClick={() => {
            setOpen((o) => !o);
          }}
        >
          {open ? "Close" : "Open"} panel
        </Button>
        <Presence show={open} recipe="panel" from="end">
          <div
            data-testid="panel"
            className="w-64 rounded-lg bg-card p-4 ring-1 ring-foreground/10"
          >
            <p className="text-sm">A panel. Its content is inert the moment it starts leaving.</p>
            <Button size="sm" variant="outline" className="mt-2" data-testid="panel-action">
              Action
            </Button>
          </div>
        </Presence>
      </div>
    );
  },
};

export const KpiEmphasis: StoryObj = {
  name: "KPI change emphasis (no count-up)",
  render: function KpiDemo() {
    const [v, setV] = useState(1204.5);
    return (
      <div className="flex flex-col gap-3">
        <PolicyReadout />
        <p className="text-3xl font-semibold tabular-nums">
          <Emphasis changeKey={v}>${v.toFixed(2)}</Emphasis>
        </p>
        <Button
          size="sm"
          className="self-start"
          data-testid="change"
          onClick={() => {
            setV((x) => x + 13.25);
          }}
        >
          New value from server
        </Button>
      </div>
    );
  },
};

export const Restored: StoryObj = {
  name: "Restored surface (history: nothing replays)",
  render: () => (
    <div className="grid gap-4 md:grid-cols-2">
      <div>
        <p className="mb-2 text-xs text-muted-foreground">Fresh (enters)</p>
        <Reveal recipe="section">
          <Card size="sm">
            <CardContent>Streamed in just now</CardContent>
          </Card>
        </Reveal>
      </div>
      <RestoredSurface>
        <div>
          <p className="mb-2 text-xs text-muted-foreground">Restored (renders in place)</p>
          <Reveal recipe="section">
            <Card size="sm">
              <CardContent>Reopened from history</CardContent>
            </Card>
          </Reveal>
        </div>
      </RestoredSurface>
    </div>
  ),
};

export const ExpressiveClamp: StoryObj = {
  name: "Showcase is clamped outside an expressive region",
  render: () => (
    <div className="grid gap-4 md:grid-cols-2">
      <div className="flex flex-col gap-2">
        <p className="text-xs text-muted-foreground">Operational surface requesting showcase</p>
        <PolicyReadout />
        <Reveal recipe="showcase">
          <Card size="sm">
            <CardContent>degrades to section</CardContent>
          </Card>
        </Reveal>
      </div>
      <ExpressiveRegion>
        <div className="flex flex-col gap-2">
          <p className="text-xs text-muted-foreground">Inside &lt;ExpressiveRegion&gt;</p>
          <PolicyReadout />
          <Reveal recipe="showcase">
            <Card size="sm">
              <CardContent>showcase allowed (still never above the user/OS)</CardContent>
            </Card>
          </Reveal>
        </div>
      </ExpressiveRegion>
    </div>
  ),
};

export const PrimitivesTransitions: StoryObj = {
  name: "Primitives: dialog, tabs (token-bounded tw-animate transitions)",
  render: () => (
    <div className="flex flex-col gap-4">
      <PolicyReadout />
      <Dialog>
        <DialogTrigger render={<Button size="sm" data-testid="open-dialog" />}>
          Open dialog
        </DialogTrigger>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Dialog</DialogTitle>
            <DialogDescription>
              Focus trap and dismissal work even if the entrance is interrupted.
            </DialogDescription>
          </DialogHeader>
        </DialogContent>
      </Dialog>
      <Tabs defaultValue="a">
        <TabsList>
          <TabsTrigger value="a">Usage</TabsTrigger>
          <TabsTrigger value="b">FinOps</TabsTrigger>
          <TabsTrigger value="c">Agents</TabsTrigger>
        </TabsList>
        <TabsContent value="a">
          Usage content (hidden tabs are not rendered, so nothing animates off-screen)
        </TabsContent>
        <TabsContent value="b">FinOps content</TabsContent>
        <TabsContent value="c">Agents content</TabsContent>
      </Tabs>
    </div>
  ),
};

export const Inventory: StoryObj = {
  render: () => (
    <table className="w-full text-left text-xs">
      <thead>
        <tr className="border-b">
          {[
            "recipe",
            "adapter",
            "trigger",
            "region",
            "token",
            "interruption",
            "reduced",
            "none",
            "cost",
            "evidence",
          ].map((h) => (
            <th key={h} className="py-1 pr-2 font-medium">
              {h}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {MOTION_INVENTORY.map((e) => (
          <tr key={e.recipe} className="border-b align-top">
            <td className="py-1 pr-2 font-mono">{e.recipe}</td>
            <td className="py-1 pr-2">{e.adapter}</td>
            <td className="py-1 pr-2">{e.trigger}</td>
            <td className="py-1 pr-2">{e.region}</td>
            <td className="py-1 pr-2">{e.token}</td>
            <td className="py-1 pr-2">{e.interruption}</td>
            <td className="py-1 pr-2">{e.reducedFallback}</td>
            <td className="py-1 pr-2">{e.noneFallback}</td>
            <td className="py-1 pr-2">{e.cost}</td>
            <td className="py-1 pr-2">{e.evidence}</td>
          </tr>
        ))}
      </tbody>
    </table>
  ),
};
