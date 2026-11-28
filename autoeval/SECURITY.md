# Grading boundary and verified limits

**This evaluator is not tamper-proof against arbitrary candidate code.** A
passing result is an observed test outcome for an ordinary patch, not a secure
proof against an adversarial program. Candidate code runs in the test framework's
process/address space or workers. It may manipulate framework internals or fake
process output. A complete defense needs stronger execution separation and
authenticated observations than generic in-process test frameworks provide.

## Reproduced vulnerability

The original runner used a world-writable `/eval/out` for framework reports and
captured logs. An otherwise unchanged buggy Python module registered an `atexit`
handler that replaced the JUnit report with passing identities, replaced the log,
and called `os._exit(0)`. The original grader incorrectly returned `resolved`.

[Before-hardening evidence](evidence/security/before/security.json) preserves the
actual exploit result, broken/fixed controls, captured output and runner hash.
This synthetic security probe is excluded from benchmark scores.

## Implemented hardening

- Candidate containers have no report/log mount or host artifact path. Reports
  are collected from Docker pipes on the host. Candidate code can append to its
  output stream but cannot seek/truncate the host's already captured bytes.
- The test command is the container's main process, so descendants do not remain
  alive in a sleeper container to rewrite files after the test command exits.
- Host capture keeps stdout/stderr separate, bounds retained bytes and enforces
  a wall-clock limit. Cargo combines streams before capture to preserve its
  binary/test ordering. Output limits, OOM and timeouts fail closed.
- The read-only pytest driver emits start, per-test setup/call/teardown, collection
  errors and finish records. The host checks complete phases, duplicate and
  unexpected records, and finish status versus the observed container exit code.
- Pytest event tokens scope output to a run. **They are not secret or an
  authentication mechanism**: candidate code shares the Python process.
- Jest/Vitest emit JSON to stdout; Go emits events; Cargo emits its ordinary
  namespaced test output. Legacy writable report-path overrides are rejected.
- Missing expected tests, zero collected tests, malformed/inconsistent records,
  skipped requirements and candidate compile failures never count as resolved.

The same file-replacement/forced-exit attack is rejected after hardening while
the broken control fails and the fixed control passes. See
[after-hardening evidence](evidence/security/after/security.json).

```sh
PYTHONPATH=autoeval/src python3 autoeval/tests/docker_security.py \
  --output /tmp/autoeval-security-new
```

The test-only `--expect-vulnerable` switch was used before the runner changed. It
does not enable an insecure execution path; it changes the regression assertion.
On the hardened runner it should fail its vulnerability expectation.

## Claims that remain unsupported

We do not claim protection against arbitrary monkeypatching, fabricated framework
events/stdout, malicious compilers/dependencies or deliberately adversarial
repository setup. Protecting artifact files is narrower than authenticating test
semantics. Hidden tests remain outside model input, but executing candidate code
inside a framework does not make those runtime tests inaccessible to that code.

The final benchmark uses a declared small set of public bug fixes and retains all
generated candidate source for review. Its checksums detect changes to the
retained files and membership; they are not independent cryptographic signatures.
