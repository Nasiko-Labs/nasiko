# Final evidence index

Only [the sealed real benchmark](../final/report.md) contributes headline model
metrics: three declared public commits, two models, one generation request per
task/model. Verify it with:

```sh
python3 autoeval/run.py verify --run autoeval/final
```

The verifier checks frozen membership and file/source hashes, recomputes metrics,
and re-parses the raw test captures. These unsigned checksums detect file changes;
they do not authenticate observations against malicious framework internals.

The remaining evidence is explicitly excluded from headline scores:

- [Five calculator validations](calculators/report.md): synthetic projects, each
  with two fail-to-pass and one existing passing test, repeated twice per code
  version. Full raw captures and normalized results are retained.
- [Security before](security/before/security.json) and
  [after](security/after/security.json): an unchanged buggy implementation forges
  the old report/log and forces exit zero. It incorrectly resolves before
  hardening and is rejected afterward; broken/fixed controls establish the probe.
  The [original runner source](security/legacy-runner.py.txt) matches the recorded
  before-test hash. The regression source is
  [docker_security.py](../tests/docker_security.py). See [the trust limits](../SECURITY.md).
- [Benchmark model pricing](pricing/source.json),
  [six-generation live ledger audit](pricing/final-ledger-audit.json), and the
  [previously approved Hydrogen/Flash additions](pricing/approved-compat.json).
  The compact [strict pricing diagnostic](pricing/strict-check.json) predates
  the final grading change and does not contribute scores.
- [Checks](checks.json): focused unit checks, source lint/format/compilation,
  Docker demonstrations and remaining host-toolchain limits.
- [Retired development artifacts](retired-development.json): 296 superseded files
  were archived in ignored `.cache` and remain recoverable from the recorded Git
  revision. They are excluded from the final review ZIP.

The final cohort retains repository Git bundles, exact parent/fix SHAs, runtime
digests, dependency inventories, task/test identities, prompts, model responses,
candidate files, raw captures and Nasiko ledger provenance. No credentials or
local repository caches are included in delivery.
