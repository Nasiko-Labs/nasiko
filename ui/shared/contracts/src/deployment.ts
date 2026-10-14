import * as z from "zod";

/**
 * Where this build runs (ADR-0001; ADR-0008: a workspace SPA served by its control plane). Decided by server configuration,
 * never by request content.
 *  - `workspace`: behind one workspace's control-plane ingress (same origin).
 *  - `central`:   the centralized multi-workspace dashboard (EE tenant-server).
 */
export const DeploymentModeSchema = z.enum(["workspace", "central"]);
export type DeploymentMode = z.infer<typeof DeploymentModeSchema>;
