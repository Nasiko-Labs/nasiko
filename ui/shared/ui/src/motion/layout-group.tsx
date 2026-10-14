"use client";

// The lab mounted motion's `LayoutGroup` around its router (common/src/app/mount.tsx) so `layoutId` morphs are
// scoped to one group across pages. The app layer may not import `motion` (FOUNDATION_ONLY_PACKAGES), so the
// foundation re-exports it here for the one app root that needs it.
export { LayoutGroup } from "motion/react";
