# AgentKV engineering constraints

- Product: workflow-aware inference optimization through Nasiko's control plane. Keep the name AgentKV.
- Follow docs/PRD.md; apply YAGNI. Do not introduce unused services, abstractions, tiers, or codecs.
- Write original project implementation. Reference repositories may be read to understand behavior and interfaces, but do not copy, vendor, or transplant their source. Do not recreate code line-by-line. Use unmodified Nasiko and necessary libraries/SDKs as declared dependencies; document attribution and versions.
- Never write credentials into source, issues, logs, tests, fixtures, or Git history. Jev credentials live in the Modal secret agentkv-jev.
- Test observable outcomes and invariants. Real cache retention requires an acknowledged engine action; never simulate performance gains in product UI.
- Keep engine/controller failures off the inference correctness path. Preserve tenant boundaries, source prompts, active state, and model/template identity.
- Pin experiment dependencies, capture raw evidence, and report uncertainty. Do not describe estimated cost as reconciled billing.
- Budget: $50 Modal ceiling, $10 reserve. Bounded experiments and explicit teardown; no unattended GPU service.
- Fetch current dependency docs with Context7: resolve the library first, then fetch task-specific docs (maximum three CLI requests per documentation question). Use official docs/source when indexing is absent and disclose the fallback. Do not put secrets in documentation queries.
