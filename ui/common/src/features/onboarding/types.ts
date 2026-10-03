/**
 * Onboarding wire types: nasiko-cloud-rs `origin/feat/user-onboarding-persona` 41f776ae, `oss/server/src/onboarding.rs`
 * (`OnboardingResponse`, `Persona`) and migration 0048 `user_persona`. Not in the OpenAPI spec (O-B3), so zod.
 */
import { z } from 'zod'

/** The server's `user_persona` enum, in its order. */
export const PERSONAS = [
  'developer',
  'platform_engineer',
  'finance',
  'engineering_manager',
  'product_manager',
  'data_analyst',
  'support_lead',
  'sre',
  'leadership',
] as const
export type Persona = (typeof PERSONAS)[number]

/** `GET`/`PATCH /api/me/onboarding`: bare JSON, no envelope. Loose: only the fields the UI reads. */
export const onboardingSchema = z.looseObject({
  is_first_time_user: z.boolean(),
  persona: z.enum(PERSONAS).nullable(),
})
export type Onboarding = z.infer<typeof onboardingSchema>
