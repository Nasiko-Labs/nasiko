// Test-only motion switch: the lab's test setup finished every animation instantly
// (`MotionGlobalConfig.skipAnimations = true`, common/src/test/setup.ts) so assertions never wait on a spring.
// Imported only by @nasiko/testing.
import { MotionGlobalConfig } from "motion/react";

export function skipAnimationsInTests(): void {
  MotionGlobalConfig.skipAnimations = true;
}
