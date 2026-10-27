/**
 * The server's password policy, `validate_password` (nasiko-cloud-rs `43833316`, oss/auth/src/lib.rs), for the
 * Settings → Account → Password form. Client-side feedback only: the server enforces it regardless. Rules are
 * checked in the server's order, so both complain about the same thing first. Unicode-aware like Rust's `char`
 * classes: lengths count code points, bytes count UTF-8 (what bcrypt reads).
 */
export const PASSWORD_MIN = 12
export const PASSWORD_MAX = 64
const PASSWORD_MAX_BYTES = 72

export type PasswordProblem =
  'bytes' | 'short' | 'long' | 'lowercase' | 'uppercase' | 'digit' | 'symbol'

/** The first rule the password breaks, or null. */
export function passwordProblem(pw: string): PasswordProblem | null {
  if (new TextEncoder().encode(pw).length > PASSWORD_MAX_BYTES) return 'bytes'
  const chars = [...pw].length
  if (chars < PASSWORD_MIN) return 'short'
  if (chars > PASSWORD_MAX) return 'long'
  if (!/\p{Lowercase}/u.test(pw)) return 'lowercase'
  if (!/\p{Uppercase}/u.test(pw)) return 'uppercase'
  if (!/\p{N}/u.test(pw)) return 'digit'
  // "Symbol" is anything neither alphabetic nor numeric, a space or an em dash included.
  if (!/[^\p{Alphabetic}\p{N}]/u.test(pw)) return 'symbol'
  return null
}

/** The server's `PasswordPolicyError::code()` for each problem (the mock answers with these). */
export const PASSWORD_CODES: Record<PasswordProblem, string> = {
  bytes: 'password_too_many_bytes',
  short: 'password_too_short',
  long: 'password_too_long',
  lowercase: 'password_missing_lowercase',
  uppercase: 'password_missing_uppercase',
  digit: 'password_missing_digit',
  symbol: 'password_missing_symbol',
}
