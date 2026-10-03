-- First-run onboarding. A user picks a persona once; the UI uses it to decide
-- which dashboard they land on. Persona is a product preference, not an access
-- grant — RBAC stays on `users.role`.
--
-- `onboarding_completed_at IS NULL` is what marks a first-time user. Existing
-- users are left NULL on purpose, so everyone is asked for a persona once.
CREATE TYPE user_persona AS ENUM (
    'developer',
    'platform_engineer',
    'finance',
    'engineering_manager',
    'product_manager',
    'data_analyst',
    'support_lead',
    'sre',
    'leadership'
);

ALTER TABLE users
    ADD COLUMN persona user_persona,
    ADD COLUMN onboarding_completed_at TIMESTAMPTZ;
