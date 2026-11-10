/**
 * CLI commands that name this server. A fresh CLI talks to the control plane built into its release
 * (`NASIKO_CLOUD_URL`, oss/cli/src/config.rs), so every guide that hands out the CLI must register this server first,
 * or `nasiko auth login` signs in somewhere else.
 */

/** This server's address for `nasiko connect`, or a placeholder where the page isn't served over http(s). */
export const serverUrl = (): string =>
  /^https?:$/.test(window.location.protocol) ? window.location.origin : 'https://<your-cluster>'

/**
 * Registers this server with the CLI and makes it the active one, without signing in: `nasiko connect` signs in on its
 * own otherwise, and the guide's own sign-in step would then ask a second time.
 */
export const connectWithoutLogin = (): string => `nasiko connect ${serverUrl()} --login=false`
