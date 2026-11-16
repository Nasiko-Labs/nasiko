# Implementation provenance

AgentKV source is written for this project. No source from the user-provided reference repositories has been copied or vendored.

References used for requirements and behavior:

- Nasiko: control-plane APIs, event structure, deployment and observability. Intended to run as an unmodified, version-pinned dependency, with original integration code where needed.
- fast-jev-compaction: narrow typed judgments, batching and explicit decisions. No compaction implementation is imported.
- TurboQuant and SpectralQuant: overhead accounting and resource-allocation research. No codec implementation is imported.
- KVFlow and PBKV: related work; no claims of reproducing or outperforming them.

Necessary libraries and SDKs remain allowed. Record exact versions in experiment manifests and review their licenses before distribution. Generated trial outputs and private task data are excluded from Git by default. There is no copied upstream code in the initial accounting primitive or Modal smoke check.

Current implementation dependencies are locked in `uv.lock`: FastAPI, Uvicorn, HTTPX, Pydantic (transitive), TypeSafe SDK and optional Modal SDK. The Nasiko hosting probe builds upstream commit `58cfe600559c67d58100ec2856d7b29838e2859f` inside a disposable Modal VM using its unmodified Dockerfile; upstream source is not added to this repository. The Nasiko reader uses the published authenticated flow endpoint's response fields. Controller, identity, scoring and Pareto logic are original project code.

The integrated implementation adds original stdlib A2A workers, request-level usage collection, a free-block eviction adapter, a benchmark fixture suite and a dependency-free browser dashboard. It uses vLLM's installed scheduler/cache interfaces; no vLLM implementation is vendored. The pinned model revision is recorded in `infra/model.py`. OpenTelemetry SDK calls provide the workers' original span instrumentation; Nasiko's deployment process installs the instrumentation dependency. Grafana Tempo is used as an unmodified tracing service.

The adapter's residency snapshot uses the engine's joint cache lookup rather than inferring recurrent-state residency from active request allocations. It includes primary block hashes and partial-prefix aliases, and validates both before reprioritizing any free group. Necessary SDK and interface knowledge is documented through official sources and Context7; copying a reference implementation is not part of this workflow.
