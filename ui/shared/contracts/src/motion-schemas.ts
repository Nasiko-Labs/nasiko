import * as z from "zod";
import { MOTION_PREFERENCES, MOTION_PROFILES, MOTION_RECIPES } from "./motion";

export const MotionRecipeSchema = z.enum(MOTION_RECIPES);
export const MotionProfileSchema = z.enum(MOTION_PROFILES);
export const MotionPreferenceSchema = z.enum(MOTION_PREFERENCES);
