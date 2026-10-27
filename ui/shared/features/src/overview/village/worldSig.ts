/** What a drawn world (3D or voxel) shows of the village, as one string: it is rebuilt only when this changes. */
import type { Village } from './model'

const r2 = (v: number) => Math.round(v * 50) / 50

/** What the kits draw from the village: places, their colours and states, the budgets' levels, the pieces' states. */
export function signature(v: Village, saved: number | null) {
  const b = v.hud.budget
  return JSON.stringify([
    v.teams.map((t) => [
      t.id,
      t.i,
      t.j,
      t.color,
      t.budget ? r2(1 - t.budget.spent / (t.budget.limit || 1)) : -1,
      (t.pace ?? 0) > 1,
    ]),
    [...v.mines, ...v.xmines].map((m) => [m.id, m.i, m.j, m.w, m.color, m.mono, m.weather]),
    v.gates.map((g) => [g.id, g.i, g.j, g.weather]),
    v.mcps.map((m) => [m.id, m.i, m.j, m.weather]),
    v.tools.map((m) => [m.id, m.i, m.j, m.weather]),
    v.cars.map((c) => [c.id, c.pose]),
    v.harnesses.map((h) => [h.kind, h.i, h.j]),
    b ? [r2(b.left / (b.limit || 1)), b.pace > 1] : null,
    !!v.outpost,
    v.log.upgrade.length > 0,
    v.workflows.some((w) => w.runs > 0),
    !!saved,
  ])
}
