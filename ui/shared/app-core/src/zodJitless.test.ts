// The page head's zod switch (./zodJitless.ts) works only while zod keeps its config on the global the script sets.
import { describe, expect, it } from 'vitest'
import { globalConfig } from 'zod/v4/core'
import { ZOD_JITLESS_SCRIPT } from './zodJitless'

describe('zod jitless switch', () => {
  it('sets the global zod reads its config from', () => {
    expect((globalThis as { __zod_globalConfig?: unknown }).__zod_globalConfig).toBe(globalConfig)
    expect(ZOD_JITLESS_SCRIPT).toBe('globalThis.__zod_globalConfig={jitless:true}')
  })
})
