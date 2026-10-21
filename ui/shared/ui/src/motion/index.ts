export * from "./policy";
export {
  MotionProvider,
  ExpressiveRegion,
  RestoredSurface,
  useMotion,
  useMotionScope,
  useOsReducedMotion,
  useHydrated,
  clampMode,
  type MotionContextValue,
  type MotionProviderProps,
  type MotionTraceEvent,
} from "./provider";
export {
  Reveal,
  StaggerGroup,
  Presence,
  Emphasis,
  type RevealProps,
  type PresenceProps,
} from "./recipes";
export { MOTION_INVENTORY, type MotionInventoryEntry } from "./inventory";
export { PortalContainerContext, usePortalContainer } from "./portal";
export { durations, ease, transitions, enter } from "./presets";
export { LayoutGroup } from "./layout-group";
export { MorphSpan } from "./morph";
export { ReorderScope } from "./reorder-scope";
// The lab's feature-level motion imports (`motion/react`): `m` components under the app's LazyMotion, presence,
// reduced-motion and the drag-to-reorder list. Features import them from here (one motion owner); the eager
// `motion` component is not re-exported (it would load the full runtime under strict LazyMotion).
export { m, AnimatePresence, Reorder, useDragControls, useReducedMotion } from "motion/react";
// The product tour's spotlight (legacy ui/ on development, dd9fe0f64): it glides a box of motion values that the dim's
// clip and the edge both read, and fades its card in by hand.
export {
  animate,
  useMotionValue,
  useTransform,
  type MotionValue,
  type Transition,
} from "motion/react";
