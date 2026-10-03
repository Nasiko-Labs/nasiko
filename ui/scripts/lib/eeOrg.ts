/**
 * The enterprise org seed (plans/feat-live-contract.md §8), created through that server's admin APIs as the superuser, so
 * the EE contract records a real org: two `seed-` users, a root unit with a child, a lead and a member, and one unit
 * grant on a seed agent. API facts at ea233d20 (ee/server/src/org_units.rs, oss/server/src/users/routes.rs):
 * - `POST /api/users` {username, email, role} → 201 {id, access_key, access_secret}; the secret is also the password.
 *   Users are hard-deleted (`DELETE /api/users/{id}`), so a seed username can be recreated at once.
 * - `POST /api/org/units` {name, parent_id?} → 201 {data: {id}}; sibling names are unique among live units.
 * - `PUT /api/org/units/{id}/lead` {lead_id} promotes a member lead to manager; `POST …/members` {user_ids}.
 * - `DELETE /api/org/units/{id}?cascade=true` soft-deletes the subtree; its names can be reused.
 * Access secrets go to `.live/credentials.json` (0600, gitignored); a user is reused only while its secret is there.
 */
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { EXIT, Failure } from './live.ts'

export interface SeedUser {
  username: string
  role: 'member'
}
export interface SeedUnit {
  name: string
  parent: string | null
  lead: string | null
  members: string[]
}

export const ORG = {
  users: [
    { username: 'seed-ee-lead', role: 'member' },
    { username: 'seed-ee-member', role: 'member' },
  ] as SeedUser[],
  units: [
    { name: 'seed-eng', parent: null, lead: 'seed-ee-lead', members: ['seed-ee-lead'] },
    { name: 'seed-eng-platform', parent: 'seed-eng', lead: null, members: ['seed-ee-member'] },
  ] as SeedUnit[],
  /** The unit granted access to agent0 (so the member reaches it through the org). */
  grantUnit: 'seed-eng',
}

interface Credential {
  id: string
  access_key: string
  access_secret: string
}
export type Credentials = Record<string, Credential>

export function readCredentials(file: string): Credentials {
  return existsSync(file) ? (JSON.parse(readFileSync(file, 'utf8')) as Credentials) : {}
}

export function writeCredentials(file: string, creds: Credentials): void {
  mkdirSync(dirname(file), { recursive: true })
  writeFileSync(file, `${JSON.stringify(creds, null, 2)}\n`, { mode: 0o600 })
  chmodSync(file, 0o600)
}

/** Which seed users to keep, recreate or create, given the server's users and the stored secrets. */
export function planUsers(
  existing: readonly { id: string; username: string }[],
  creds: Credentials,
): { keep: string[]; recreate: { username: string; id: string }[]; create: string[] } {
  const out = {
    keep: [] as string[],
    recreate: [] as { username: string; id: string }[],
    create: [] as string[],
  }
  for (const u of ORG.users) {
    const live = existing.find((x) => x.username === u.username)
    if (!live) out.create.push(u.username)
    else if (creds[u.username]?.id === live.id) out.keep.push(u.username)
    else out.recreate.push({ username: u.username, id: live.id })
  }
  return out
}

/** Units in creation order (parents first). */
export function unitOrder(units: readonly SeedUnit[] = ORG.units): SeedUnit[] {
  const done = new Set<string>()
  const out: SeedUnit[] = []
  while (out.length < units.length) {
    const next = units.find((u) => !done.has(u.name) && (!u.parent || done.has(u.parent)))
    if (!next)
      throw new Failure('ee-org', 'the seed org has a unit whose parent is missing', EXIT.usage)
    out.push(next)
    done.add(next.name)
  }
  return out
}

type Api = (
  method: string,
  path: string,
  body?: unknown,
) => Promise<{ status: number; body: unknown }>

/** An authenticated JSON client for one server and token. */
export function apiClient(base: string, token: string): Api {
  return async (method, path, body) => {
    const res = await fetch(`${base}${path}`, {
      method,
      headers: {
        Authorization: `Bearer ${token}`,
        Accept: 'application/json',
        ...(body === undefined ? {} : { 'Content-Type': 'application/json' }),
      },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(15_000),
    })
    const text = await res.text()
    let parsed: unknown = text
    try {
      parsed = text ? JSON.parse(text) : null
    } catch {
      // Plain-text error bodies stay text.
    }
    return { status: res.status, body: parsed }
  }
}

const expect = (r: { status: number; body: unknown }, ok: number[], what: string) => {
  if (!ok.includes(r.status))
    throw new Failure(
      'ee-org',
      `${what} answered ${r.status}: ${JSON.stringify(r.body).slice(0, 200)}`,
      EXIT.notReady,
      'npm run ee:server   (is the EE server running?)',
    )
}

interface UnitRow {
  id: string
  name: string
  parent_id: string | null
}
const data = <T>(b: unknown): T =>
  b && typeof b === 'object' && 'data' in b ? (b as { data: T }).data : (b as T)

/**
 * Refuse to write org rows unless this server holds the seed marker: the org writes go to `--url`, which could name any
 * localhost server (the dev server on :8080 included), while the SQL seed went to nasiko_ee. With `anchor`, the marker
 * must be this run's (`seed-marker-<anchor>`); without it (a reset), any seed marker proves a seed-only database.
 */
export async function requireSeedMarker(api: Api, anchor?: string): Promise<void> {
  const r = await api('GET', '/api/llm-configs')
  const names =
    r.status === 200
      ? (data<{ name?: unknown }[]>(r.body) ?? []).map((c) => String(c.name ?? ''))
      : []
  const ok = anchor
    ? names.includes(`seed-marker-${anchor}`)
    : names.some((n) => n.startsWith('seed-marker-'))
  if (!ok)
    throw new Failure(
      'ee-org',
      `refusing to write the EE org: the server has no ${anchor ? `seed-marker-${anchor}` : 'seed marker'} (is --url the EE server on nasiko_ee?)`,
      EXIT.notReady,
      'npm run ee:server   (then npm run seed:ee against http://127.0.0.1:9090)',
    )
}

/** Create (or bring back in line) the seed org. Returns the credentials of every seed user. */
export async function seedOrg(api: Api, credsFile: string, agent0: string): Promise<Credentials> {
  const creds = readCredentials(credsFile)
  const list = await api('GET', '/api/users?q=seed-ee-&limit=100')
  expect(list, [200], 'GET /api/users')
  const plan = planUsers(data<{ id: string; username: string }[]>(list.body) ?? [], creds)
  for (const { username, id } of plan.recreate) {
    expect(await api('DELETE', `/api/users/${id}`), [204], `DELETE /api/users (${username})`)
    delete creds[username]
  }
  for (const username of [...plan.create, ...plan.recreate.map((r) => r.username)]) {
    const u = ORG.users.find((x) => x.username === username)!
    const r = await api('POST', '/api/users', {
      username,
      email: `${username}@example.com`,
      role: u.role,
    })
    expect(r, [201], `POST /api/users (${username})`)
    const b = r.body as { id: string; access_key: string; access_secret: string }
    creds[username] = { id: b.id, access_key: b.access_key, access_secret: b.access_secret }
    writeCredentials(credsFile, creds)
  }

  const unitsRes = await api('GET', '/api/org/units')
  expect(unitsRes, [200], 'GET /api/org/units')
  const units = data<UnitRow[]>(unitsRes.body) ?? []
  const ids = new Map<string, string>()
  for (const u of unitOrder()) {
    const parentId = u.parent ? ids.get(u.parent)! : null
    const live = units.find((x) => x.name === u.name && x.parent_id === parentId)
    let id = live?.id
    if (!id) {
      const r = await api('POST', '/api/org/units', {
        name: u.name,
        ...(parentId ? { parent_id: parentId } : {}),
      })
      expect(r, [201], `POST /api/org/units (${u.name})`)
      id = data<{ id: string }>(r.body).id
    }
    ids.set(u.name, id)
    if (u.lead)
      expect(
        await api('PUT', `/api/org/units/${id}/lead`, { lead_id: creds[u.lead]!.id }),
        [200, 204],
        `PUT lead (${u.name})`,
      )
    if (u.members.length)
      expect(
        await api('POST', `/api/org/units/${id}/members`, {
          user_ids: u.members.map((m) => creds[m]!.id),
        }),
        [200],
        `POST members (${u.name})`,
      )
  }
  // 201 when new; an existing grant is a conflict, which is fine on a re-run.
  expect(
    await api('POST', `/api/agents/${agent0}/grants/units/${ids.get(ORG.grantUnit)}`),
    [200, 201, 409],
    'POST unit grant',
  )
  return creds
}

/** Remove the seed org: root units (cascade), then the seed users, then the stored secrets. */
export async function resetOrg(api: Api, credsFile: string): Promise<void> {
  const unitsRes = await api('GET', '/api/org/units')
  if (unitsRes.status === 200) {
    for (const u of (data<UnitRow[]>(unitsRes.body) ?? []).filter(
      (x) => !x.parent_id && ORG.units.some((s) => !s.parent && s.name === x.name),
    )) {
      expect(
        await api('DELETE', `/api/org/units/${u.id}?cascade=true`),
        [200, 404],
        `DELETE unit ${u.name}`,
      )
    }
  }
  const list = await api('GET', '/api/users?q=seed-ee-&limit=100')
  for (const u of (list.status === 200
    ? (data<{ id: string; username: string }[]>(list.body) ?? [])
    : []
  ).filter((x) => ORG.users.some((s) => s.username === x.username))) {
    expect(await api('DELETE', `/api/users/${u.id}`), [204, 404], `DELETE user ${u.username}`)
  }
  rmSync(credsFile, { force: true })
}

/** Where `npm run ee:server` records the nasiko-cloud-rs commit its running binary was built from. */
export const EE_BUILD_FILE = join(
  dirname(fileURLToPath(import.meta.url)),
  '..',
  '..',
  '.live',
  'ee-build.json',
)

/** The commit the running EE server was built from, or null when ee:server hasn't recorded one. */
export function readEeBuild(file = EE_BUILD_FILE): string | null {
  if (!existsSync(file)) return null
  try {
    const sha = (JSON.parse(readFileSync(file, 'utf8')) as { sha?: unknown }).sha
    return typeof sha === 'string' && /^[0-9a-f]{40}$/.test(sha) ? sha : null
  } catch {
    return null // a truncated or hand-edited file
  }
}
