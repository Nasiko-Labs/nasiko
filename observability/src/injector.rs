use std::collections::HashMap;

/// The Python sitecustomize file that is embedded into agent images at build time.
///
/// Python runs `sitecustomize.py` automatically before any user code, so this
/// file installs the session.id wrapper without any changes to the agent source.
pub const OTEL_PATCH_PY: &str = r#"# Injected by Nasiko as sitecustomize.py — Python runs this file automatically
# before any user code is imported, so no agent source changes are needed.
#
# What this does:
#   1. Initialises the OpenTelemetry SDK (same effect as `opentelemetry-instrument`
#      wrapper, but done here so we control the load order).
#   2. Patches uvicorn.run to wrap the ASGI app with a lightweight middleware that
#      extracts the W3C traceparent header from each request and stores the resulting
#      OTel context in a ContextVar.  This gives every asyncio Task spawned from the
#      request handler (including the a2a-sdk background task for execute()) access
#      to the per-request trace context without any agent code changes.
#   3. Hooks into AgentExecutor.__init_subclass__ so that every class which inherits
#      from AgentExecutor gets its execute() method wrapped with an OTel span that:
#        - reads the per-request context from the ContextVar (step 2) to ensure each
#          incoming message creates a NEW trace in Tempo, not a continuation of an
#          old one (fixes OTel context leakage in async ASGI apps)
#        - sets session.id = A2A contextId so the Nasiko dashboard can group all
#          messages in a conversation into a single session
#
#   opentelemetry-instrumentation-openai (loaded by initialize()) then
#   auto-instruments OpenAI SDK calls and creates child spans under this root span,
#   giving full token counts, latency, and prompt/completion content — all with
#   zero OTel code in the agent source.
#
# NOTE: We intentionally do NOT use `opentelemetry-instrument` as a CMD wrapper
# because that prepends its own sitecustomize.py to PYTHONPATH, which would
# shadow this file and prevent the AgentExecutor patch from running.

import contextvars
import functools
import logging

_log = logging.getLogger("nasiko.otel_patch")

# ContextVar populated by _TraceparentMiddleware for each incoming HTTP request.
# asyncio copies ContextVars into child Tasks at creation time, so execute()
# (which runs in a background Task spawned from the request handler) always
# sees the context that was active when its Task was created.
_request_otel_ctx: contextvars.ContextVar = contextvars.ContextVar(
    "_nasiko_request_otel_ctx",
    default=None,
)


class _TraceparentMiddleware:
    """Raw ASGI middleware — no response buffering, safe for SSE streaming."""

    def __init__(self, app):
        self.app = app

    async def __call__(self, scope, receive, send):
        if scope["type"] != "http":
            await self.app(scope, receive, send)
            return

        # Decode headers from bytes; ASGI spec guarantees latin-1.
        headers: dict = {}
        for name_b, value_b in scope.get("headers", []):
            try:
                headers[name_b.decode("latin-1").lower()] = value_b.decode("latin-1")
            except Exception:
                pass

        try:
            from opentelemetry.propagate import extract as _extract
            ctx = _extract(headers)
        except Exception:
            try:
                from opentelemetry import context as _otel_ctx
                ctx = _otel_ctx.Context()
            except Exception:
                ctx = None

        token = _request_otel_ctx.set(ctx)
        try:
            await self.app(scope, receive, send)
        finally:
            _request_otel_ctx.reset(token)


def _install():
    # ── Step 1: Initialise OTel SDK ──────────────────────────────────────────
    # Calling initialize() here replicates what `opentelemetry-instrument`
    # does, but without the PYTHONPATH injection that would shadow this file.
    try:
        from opentelemetry.instrumentation.auto_instrumentation import initialize
        initialize()
        _log.debug("nasiko: OTel auto-instrumentation initialized")
    except Exception as exc:
        _log.debug("nasiko: OTel init skipped: %s", exc)

    # ── Step 2: Patch uvicorn.run to inject TraceparentMiddleware ─────────────
    # This must happen before the agent imports uvicorn, but sitecustomize.py
    # runs before any user imports so the timing is guaranteed.
    try:
        import uvicorn as _uvicorn
        _orig_run = _uvicorn.run

        def _patched_run(app, **kwargs):
            return _orig_run(_TraceparentMiddleware(app), **kwargs)

        _uvicorn.run = _patched_run
        _log.debug("nasiko: uvicorn.run patched with TraceparentMiddleware")
    except Exception as exc:
        _log.debug("nasiko: uvicorn patch skipped: %s", exc)

    # ── Step 3: Patch AgentExecutor to set session.id on every execute() ─────
    try:
        from a2a.server.agent_execution import AgentExecutor
        from opentelemetry import context as otel_context
        from opentelemetry import trace
    except ImportError:
        # a2a-sdk or OTel not installed in this image — skip silently.
        return

    _original_isc = AgentExecutor.__init_subclass__

    def _wrap_execute(cls):
        """Replace cls.execute with an instrumented version (idempotent)."""
        if "execute" not in cls.__dict__:
            return

        _original = cls.__dict__["execute"]

        # Guard against double-wrapping (e.g. agent already has its own OTel code).
        if getattr(_original, "_nasiko_instrumented", False):
            return

        @functools.wraps(_original)
        async def _instrumented(self, context, event_queue):
            # Resolve the A2A task to get the session (context) ID.
            try:
                from a2a.helpers import new_task_from_user_message

                task = context.current_task or new_task_from_user_message(
                    context.message
                )
                session_id = task.context_id
            except Exception:
                # If session resolution fails, run the original unmodified.
                return await _original(self, context, event_queue)

            tracer = trace.get_tracer("nasiko.agent")

            # Use the W3C context extracted from the HTTP request by
            # _TraceparentMiddleware (step 2).  asyncio copies ContextVars into
            # child Tasks, so this value is the one from the specific request that
            # spawned this execute() Task — not leaked from a previous request.
            # Falling back to an empty Context() ensures a fresh root span when
            # no traceparent header was present (e.g. direct agent card fetches).
            parent_ctx = _request_otel_ctx.get()
            if parent_ctx is None:
                parent_ctx = otel_context.Context()

            # start_as_current_span with an explicit context= means the span is
            # parented under the incoming request's trace rather than whatever
            # context happens to be active in the asyncio event loop.  This is
            # what prevents all requests from landing in the same Tempo trace.
            with tracer.start_as_current_span("agent.request", context=parent_ctx) as span:
                span.set_attribute("session.id", session_id)
                return await _original(self, context, event_queue)

        _instrumented._nasiko_instrumented = True
        cls.execute = _instrumented
        _log.debug("nasiko: wrapped execute() on %s", cls.__name__)

    @classmethod
    def _patched_init_subclass(cls, **kwargs):
        # Forward to the original __init_subclass__ first.
        try:
            _original_isc.__func__(cls, **kwargs)
        except (TypeError, AttributeError):
            pass
        _wrap_execute(cls)

    AgentExecutor.__init_subclass__ = _patched_init_subclass
    _log.debug("nasiko: AgentExecutor.__init_subclass__ patched")


_install()
"#;

/// Write [`OTEL_PATCH_PY`] into `build_dir` as `.nasiko_otel_patch.py` so the
/// patched Dockerfile's `COPY` instruction can include it in the image.
///
/// The leading dot keeps the file out of the way of agent source files and makes
/// it clear it was injected by the platform, not written by the agent developer.
pub fn write_otel_patch_file(build_dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(build_dir.join(".nasiko_otel_patch.py"), OTEL_PATCH_PY)
}

/// Patch a Dockerfile to add OpenTelemetry auto-instrumentation.
///
/// Supports Python agents. Detects the runtime from the Dockerfile content and:
/// 1. Inserts a `RUN pip install` step for the OTel packages + bootstrap before the CMD.
/// 2. Copies the Nasiko session.id patch into the image as `sitecustomize.py`
///    so it runs before any agent code — enabling session grouping in the dashboard
///    without any OTel code in the agent source.
/// 3. Wraps the CMD with `opentelemetry-instrument` so traces are emitted
///    automatically.
///
/// Idempotent: if `opentelemetry-instrument` is already present, the Dockerfile is
/// returned unchanged.
pub fn patch_dockerfile_for_otel(content: &str) -> String {
    // Already patched — don't double-inject.
    if content.contains(".nasiko_otel_patch.py") {
        return content.to_string();
    }

    let is_python = content.contains("pip install")
        || content.contains("pip3 install")
        || content.to_lowercase().contains("python");

    if !is_python {
        // Non-Python runtimes: return unchanged for now.
        return content.to_string();
    }

    // Three Dockerfile instructions appended before the CMD:
    //   1. pip install — OTel SDK + auto-instrumentation packages
    //   2. COPY       — bring .nasiko_otel_patch.py into the image
    //   3. RUN        — install it as sitecustomize.py using Python's own site
    //                   module so the path is correct for any Python version
    //
    // NOTE: We intentionally do NOT wrap CMD with `opentelemetry-instrument`.
    // That wrapper prepends its own sitecustomize.py to PYTHONPATH, which would
    // shadow our sitecustomize.py and prevent the AgentExecutor patch from
    // running.  Our sitecustomize.py calls
    // `opentelemetry.instrumentation.auto_instrumentation.initialize()` directly,
    // achieving the same effect without the PYTHONPATH shadowing issue.
    let otel_install = concat!(
        "RUN pip install --no-cache-dir opentelemetry-distro opentelemetry-exporter-otlp ",
        "&& opentelemetry-bootstrap -a install\n",
        "COPY .nasiko_otel_patch.py /tmp/.nasiko_otel_patch.py\n",
        "RUN python3 -c \"import site,shutil,os;",
        "[shutil.copy('/tmp/.nasiko_otel_patch.py',os.path.join(d,'sitecustomize.py'))",
        " for d in site.getsitepackages() if os.path.isdir(d)]\"",
        " 2>/dev/null || true"
    );

    // Insert the OTel install block just before the CMD — no CMD rewrite needed.
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    let mut cmd_index: Option<usize> = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim().to_uppercase();
        if trimmed.starts_with("CMD") {
            cmd_index = Some(i);
            break;
        }
    }

    let Some(cmd_idx) = cmd_index else {
        // No CMD found — just append the install block.
        lines.push(otel_install.to_string());
        return lines.join("\n");
    };

    // Insert the OTel install block just before CMD; leave CMD unchanged.
    lines.insert(cmd_idx, otel_install.to_string());
    lines.join("\n")
}

/// Context passed to [`InstrumentationInjector::inject`] when deploying an agent.
pub struct AgentContext {
    /// The agent's stable identifier (used as `OTEL_SERVICE_NAME`).
    pub agent_id: String,
    /// Optional tenant identifier, added to `OTEL_RESOURCE_ATTRIBUTES`.
    pub tenant_id: Option<String>,
    /// Optional image version, added to `OTEL_RESOURCE_ATTRIBUTES` as `service.version`.
    pub version: Option<String>,
    /// Whether to capture prompt/completion content in logs
    /// (`OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT`).
    pub capture_content: bool,
    /// OTLP collector endpoint.
    pub otel_collector_endpoint: String,
    /// OTLP export protocol (`grpc` or `http/protobuf`).
    pub otel_protocol: String,
}

/// Injects OpenTelemetry environment variables into an agent's `env_vars` map
/// at deploy time so the agent is automatically instrumented without code changes.
pub trait InstrumentationInjector: Send + Sync {
    fn inject(&self, env_vars: &mut HashMap<String, String>, ctx: &AgentContext);
}

/// OSS implementation: injects the 7 standard `OTEL_*` env vars.
pub struct OtelInjector;

impl InstrumentationInjector for OtelInjector {
    fn inject(&self, env_vars: &mut HashMap<String, String>, ctx: &AgentContext) {
        env_vars.insert(
            "OTEL_EXPORTER_OTLP_ENDPOINT".into(),
            ctx.otel_collector_endpoint.clone(),
        );
        env_vars.insert(
            "OTEL_EXPORTER_OTLP_PROTOCOL".into(),
            ctx.otel_protocol.clone(),
        );
        env_vars.insert("OTEL_SERVICE_NAME".into(), ctx.agent_id.clone());

        let mut resource_attrs = format!("agent.id={}", ctx.agent_id);
        if let Some(tenant) = &ctx.tenant_id {
            resource_attrs.push_str(&format!(",tenant.id={tenant}"));
        }
        if let Some(version) = &ctx.version {
            resource_attrs.push_str(&format!(",service.version={version}"));
        }
        env_vars.insert("OTEL_RESOURCE_ATTRIBUTES".into(), resource_attrs);

        env_vars.insert("OTEL_TRACES_EXPORTER".into(), "otlp".into());
        env_vars.insert("OTEL_LOGS_EXPORTER".into(), "otlp".into());
        // opentelemetry-instrumentation-openai-v2 uses an enum instead of a boolean:
        //   NO_CONTENT  — no prompt/completion captured (opt-out for compliance)
        //   EVENT_ONLY  — content emitted as OTel log events → Loki
        //   SPAN_ONLY   — content added as span attributes → Tempo
        //   SPAN_AND_EVENT — both
        // Older instrumentation packages accept "true"/"false"; we use the enum
        // value so both old and new packages produce a useful result.
        env_vars.insert(
            "OTEL_INSTRUMENTATION_GENAI_CAPTURE_MESSAGE_CONTENT".into(),
            if ctx.capture_content {
                "EVENT_ONLY"
            } else {
                "NO_CONTENT"
            }
            .into(),
        );
    }
}
