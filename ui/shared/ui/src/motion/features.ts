// Loaded lazily by MotionProvider's LazyMotion. `domAnimation` is the smallest
// feature set that covers opacity/transform recipes and AnimatePresence; the
// layout/drag feature set (`domMax`, ./features-max.ts) loads only where a root opts in with
// `features="max"` (the app scope hosting the ported lab screens).
import { domAnimation } from "motion/react";

export default domAnimation;
