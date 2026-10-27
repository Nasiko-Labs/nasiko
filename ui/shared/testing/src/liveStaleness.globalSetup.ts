/**
 * Vitest global setup: print the live-fixture staleness warning once per run (scripts/lib/staleness.ts, plan §7.6).
 * Information only: it never fails a run and is silent without a sibling nasiko-cloud-rs checkout (CI).
 */
import { join, resolve } from 'node:path'
import { stalenessWarning } from '../../../scripts/lib/staleness.ts'

export default function setup(): void {
  try {
    const warning = stalenessWarning(
      join(__dirname, '__live__'),
      resolve(process.env.NASIKO_CLOUD_RS ?? join(__dirname, '../../../../nasiko-cloud-rs')),
    )
    if (warning) console.warn(`\n${warning}\n`)
  } catch {
    // A warning must never break the test run.
  }
}
