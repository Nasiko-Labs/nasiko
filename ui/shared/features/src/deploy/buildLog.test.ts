import { describe, expect, it } from 'vitest'
import { buildLogLines } from './buildLog'

describe('buildLogLines', () => {
  it('strips ANSI color escapes so they do not render as literal text', () => {
    const line = buildLogLines('\u001b[91m   Compiling foo\u001b[0m')[0]!
    expect(line.text).toBe('   Compiling foo')
  })

  it('flags lines that report a failure, leaving ordinary lines alone', () => {
    const lines = buildLogLines(
      [
        'Step 1/12 : FROM rust:1-alpine AS builder',
        'Successfully tagged hr-agent:0.1.1',
        'COPY failed: file does not exist',
        'error[E0432]: unresolved import',
        'The command returned a non-zero code: 1',
      ].join('\n'),
    )
    expect(lines.map((l) => l.error)).toEqual([false, false, true, true, true])
  })

  it('does not flag a clean summary or an ordinary path', () => {
    // "errors: 0" (plural) is a success summary, not an error line.
    expect(buildLogLines('warnings: 0, errors: 0')[0]!.error).toBe(false)
    // "failed" inside a path word must not trip it.
    expect(buildLogLines('/app/src/failedtests/mod.rs')[0]!.error).toBe(false)
  })

  it('preserves blank lines as entries', () => {
    expect(buildLogLines('a\n\nb')).toHaveLength(3)
  })

  it('strips the control-plane error-wrapper prefixes so the log opens on the first real line', () => {
    const raw =
      'docker build: internal error: Step 1/3 : FROM alpine\nCOPY failed: file does not exist'
    const lines = buildLogLines(raw)
    expect(lines[0]!.text).toBe('Step 1/3 : FROM alpine')
    // The FROM step is not an error line; only the COPY failure is red.
    expect(lines[0]!.error).toBe(false)
    expect(lines[1]!).toEqual({ text: 'COPY failed: file does not exist', error: true })
  })

  it('strips a bare internal-error prefix (the registry/marketplace import path)', () => {
    expect(buildLogLines('internal error: COPY failed: x')[0]!.text).toBe('COPY failed: x')
  })
})
