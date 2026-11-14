import { apiUrl, isApiBaseUnresolved } from '@nasiko/data-access/client'

/**
 * CLI commands that name this server. A fresh CLI talks to the control plane built into its release
 * (`NASIKO_CLOUD_URL`, oss/cli/src/config.rs), so every guide that hands out the CLI must register this server first,
 * or `nasiko auth login` signs in somewhere else.
 */

/** Hosts whose page is the management plane; their Free workspace is `free.<host>`. */
const MANAGEMENT_HOSTS = new Set(['app.nasiko.com'])

/**
 * The control plane for a page and the origin its API calls go to. That is the API base, not the page: the multi-tenant
 * dashboard's page is the management plane, its API the workspace. With no API base set yet, a management host falls
 * back to its Free workspace.
 */
export const controlPlaneUrl = (page: URL, api: URL): string =>
  api.origin === page.origin && MANAGEMENT_HOSTS.has(page.hostname)
    ? `${page.protocol}//free.${page.host}`
    : api.origin

/** The address for `nasiko connect`, or a placeholder where the page isn't served over http(s) or the API base is unknown. */
export const serverUrl = (): string =>
  /^https?:$/.test(window.location.protocol) && !isApiBaseUnresolved()
    ? controlPlaneUrl(new URL(window.location.href), apiUrl('/'))
    : 'https://<your-cluster>'

/**
 * Registers this server with the CLI and makes it the active one, without signing in: `nasiko connect` signs in on its
 * own otherwise, and the guide's own sign-in step would then ask a second time.
 */
export const connectWithoutLogin = (): string => `nasiko connect ${serverUrl()} --login=false`
