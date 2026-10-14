// Ported from nasiko-ui-lab@1b99bdda common/src/lib/motionFeatures.ts. Local edits: none beyond the comment.
// Loaded lazily by MotionProvider's LazyMotion when a root asks for `features="max"`: the lab's bundle
// (`domMax`: everything in `domAnimation` plus layout animations and drag), for the app scope that hosts the
// ported lab screens whose `layoutId` morphs need the layout features. Every other scope keeps `./features`.
import { domMax } from "motion/react";

export default domMax;
