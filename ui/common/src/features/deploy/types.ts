/**
 * Wire types for builds and uploads (plans/feat-deploy.md §8), from nasiko-cloud-rs `2d6178e4`.
 *
 * `/api/builds*` is not in the server's OpenAPI spec (server gap D-1), so its shapes are zod schemas here, checked by the
 * live contract. Upload and deploy-stream types come from `schema.gen.ts`.
 */
import { z } from 'zod'
import type { components } from '@/lib/api/schema.gen'

/** `build/mod.rs` `BuildStatus` (PG enum `build_status`). */
export const BUILD_STATUSES = ['queued', 'building', 'success', 'failed'] as const
export type BuildStatus = (typeof BUILD_STATUSES)[number]

/** `build/routes.rs` `BuildRecord`: `logs_url` is never written by the OSS server (D-3). */
export const buildRecordSchema = z.object({
  id: z.string(),
  agent_id: z.string(),
  github_url: z.string().nullable(),
  commit_hash: z.string().nullable(),
  version_tag: z.string(),
  image_reference: z.string(),
  status: z.enum(BUILD_STATUSES),
  logs_url: z.string().nullable(),
  created_at: z.string(),
  updated_at: z.string(),
})
export type BuildRecord = z.infer<typeof buildRecordSchema>

/** `lib.rs` `Paginated<BuildRecord>`: `total` is the page length, not a count (D-6). */
export const buildsPageSchema = z.object({ data: z.array(buildRecordSchema), total: z.number() })
export type BuildsPage = z.infer<typeof buildsPageSchema>

/** `crate::unavailable()`: a 200 `{available:false}` for a caller without deploy rights (EE; OSS lets everyone deploy). */
export const unavailableSchema = z.object({ available: z.literal(false) })

/** One frame of `GET /api/agents/deploys/{id}/stream` (`agents/upload.rs` `deploy_status_sse`): sent on change only. */
export const streamFrameSchema = z.object({
  status: z.enum([...BUILD_STATUSES, 'not_found']),
  build_id: z.string().optional(),
})

/** `upload.rs` `UploadStatusItem` (bare on `/uploads/{id}`, enveloped on the list). */
export type UploadStatus = components['schemas']['UploadStatusItem']
/** `upload_pipeline_status`: initiated → processing → orchestration_triggered → orchestration_processing → completed | failed. */
export type UploadPipelineStatus =
  | 'initiated'
  | 'processing'
  | 'capabilities_generated'
  | 'orchestration_triggered'
  | 'orchestration_processing'
  | 'completed'
  | 'failed'

// ── GitHub (`github.rs`; not in the OpenAPI spec, D-1) ────────────────────────────────────────

/** `GET /api/auth/github/status` (public): whether the server has a GitHub OAuth app. */
export const githubConfiguredSchema = z.object({ configured: z.boolean() })

/** `GET /api/github/user` (`github_status`): `configured:false` without an OAuth app; `valid` checks the stored token. */
export const githubUserSchema = z.object({
  connected: z.boolean(),
  valid: z.boolean().optional(),
  configured: z.boolean().optional(),
  login: z.string().nullable().optional(),
})
export type GithubUser = z.infer<typeof githubUserSchema>

/**
 * `GET /api/auth/github/token` (`github_token`): the raw token is never returned. Connected is a 200 with
 * `status: connected|invalid`; not connected is a **202** `{success:false, status:"disconnected"}`; no OAuth app is a 503.
 */
export const githubTokenSchema = z.object({
  success: z.boolean(),
  status: z.enum(['connected', 'invalid', 'disconnected']),
  message: z.string().optional(),
  username: z.string().nullable().optional(),
})

/** `nasiko_github` `GitHubRepo`, via `GET /api/github/repositories` → `{repositories, total}` (the newest 100). */
export const githubRepoSchema = z.object({
  id: z.number(),
  name: z.string(),
  full_name: z.string(),
  description: z.string().nullable().optional(),
  private: z.boolean(),
  html_url: z.string(),
  default_branch: z.string(),
  updated_at: z.string(),
})
export type GithubRepo = z.infer<typeof githubRepoSchema>
export const githubReposSchema = z.object({
  repositories: z.array(githubRepoSchema),
  total: z.number(),
})

/** `POST /api/github/clone` → 202 `CloneResult`; `upload_id` is the build id. */
export const cloneResultSchema = z.object({
  success: z.boolean(),
  message: z.string(),
  agent_name: z.string().nullable(),
  upload_id: z.string().nullable(),
})
