import { describe, expect, it } from 'vitest'
import { passwordProblem } from './password'

describe('passwordProblem (nasiko_auth::validate_password)', () => {
  it('reports the first broken rule in the server order', () => {
    expect(passwordProblem('A-brand-new-password9')).toBeNull()
    expect(passwordProblem('Short-pw9')).toBe('short')
    expect(passwordProblem(`Aa9-${'x'.repeat(61)}`)).toBe('long')
    expect(passwordProblem('alllowercase-9')).toBe('uppercase')
    expect(passwordProblem('ALLUPPERCASE-9')).toBe('lowercase')
    expect(passwordProblem('NoDigitsHere-x')).toBe('digit')
    expect(passwordProblem('NoSymbolsHere9')).toBe('symbol')
  })

  it('is Unicode-aware like Rust, and checks bytes before length', () => {
    // A space and an em dash are symbols; Greek letters have case.
    expect(passwordProblem('Πάσσωορδ λ—9Ab')).toBeNull()
    // 22 code points but 76 bytes (each emoji is 4): bcrypt reads 72, so the server refuses rather than truncating.
    expect(passwordProblem(`Aa9-${'😀'.repeat(18)}`)).toBe('bytes')
  })
})
