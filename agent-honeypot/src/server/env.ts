/**
 * Minimal .env loader — reads the Nasiko root `.env` (../.env from this repo)
 * plus a local `.env` if present, WITHOUT overwriting already-set vars.
 *
 * We deliberately avoid a dependency: the honeypot needs only a handful of vars
 * (JWT_SECRET, ADMIN_PASSWORD, NASIKO_BASE_URL, …) that live in the Nasiko root
 * `.env` the user already configured for the quickstart.
 */
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

function load(path: string) {
  if (!existsSync(path)) return;
  for (const raw of readFileSync(path, "utf8").split("\n")) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const eq = line.indexOf("=");
    if (eq === -1) continue;
    const key = line.slice(0, eq).trim();
    let val = line.slice(eq + 1).trim();
    if (
      (val.startsWith('"') && val.endsWith('"')) ||
      (val.startsWith("'") && val.endsWith("'"))
    ) {
      val = val.slice(1, -1);
    }
    if (process.env[key] === undefined) process.env[key] = val;
  }
}

// Nasiko root .env (…/nasiko/.env) — two levels up from src/server/.
load(resolve(here, "../../../.env"));
// Local override, if the user adds one.
load(resolve(here, "../../.env"));

/**
 * Map Nasiko's `ADMIN_PASSWORD`/`ADMIN_USERNAME` to the names the setup script
 * expects, so no duplicate config is needed.
 */
if (!process.env.NASIKO_ADMIN_PASS && process.env.ADMIN_PASSWORD) {
  process.env.NASIKO_ADMIN_PASS = process.env.ADMIN_PASSWORD;
}
if (!process.env.NASIKO_ADMIN_USER && process.env.ADMIN_USERNAME) {
  process.env.NASIKO_ADMIN_USER = process.env.ADMIN_USERNAME;
}
