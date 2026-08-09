/**
 * Shared reader for A2A `message/stream` SSE responses.
 *
 * One parser for every frame shape the proxy can forward (see
 * `normalize_agent_event` in oss/server/src/router/a2a_dispatch.rs):
 *
 *  - `statusUpdate` / `result.statusUpdate` — task status frames. WORKING
 *    text is progress prose, COMPLETED text is the authoritative reply.
 *  - `artifactUpdate` / `result.artifactUpdate` — reply text chunks
 *    (append or replace mode, `lastChunk` marks the final one).
 *  - flat `message` / `result.message` — 0.3.x agents reply with a bare
 *    Message instead of task frames.
 *  - `task` / `result.task` — task-wrapped replies carrying `artifacts`
 *    and/or `status.message`.
 *  - `error` — JSON-RPC error frames.
 *
 * Callers receive semantic callbacks and never touch frame shapes:
 *
 *   const out = await readA2aStream(res, {
 *     onProgress(text)  {}  // cumulative working/progress prose
 *     onActivity(line)  {}  // one new line of working prose (tool activity)
 *     onReply(text)     {}  // cumulative reply text (call renders it)
 *     onData(part)      {}  // data parts (agent-steps events)
 *     onTraceMeta(meta) {}  // { trace_id }
 *     onUsageMeta(meta) {}  // usage footer (tokens/cost), when present
 *     onError(message)  {}  // stream-level failure text
 *   });
 *   // out = { text, progressText, traceId, usage, failed, errorMessage }
 */

function textOfParts(parts) {
  if (!Array.isArray(parts)) return "";
  return parts.filter((p) => p && p.text).map((p) => p.text).join("");
}

/** Accumulate working text from senders that mix cumulative + delta styles. */
function mergeProgress(accumulated, incoming) {
  if (!incoming) return accumulated;
  if (incoming.startsWith(accumulated)) return incoming; // cumulative re-send
  return accumulated + incoming; // delta
}

export async function readA2aStream(res, handlers = {}) {
  const out = {
    text: "",
    progressText: "",
    traceId: null,
    usage: null,
    failed: false,
    errorMessage: null,
  };
  // Tracked separately from out.progressText: activity is reported for the
  // whole stream, progressText only until a reply exists.
  let activitySeen = "";
  const emitReply = () => {
    if (out.text) handlers.onReply?.(out.text);
  };

  const handleDataParts = (parts) => {
    for (const part of parts || []) {
      if (!part || !part.data) continue;
      const d = part.data;
      if (d.type === "trace_meta" && d.trace_id) {
        out.traceId = d.trace_id;
        handlers.onTraceMeta?.(d);
        continue;
      }
      if (d.type === "usage_meta") {
        out.usage = d;
        handlers.onUsageMeta?.(d);
        continue;
      }
      handlers.onData?.(d);
    }
  };

  const handleMessage = (msg) => {
    // A flat agent Message is a complete reply.
    if (!msg) return;
    const text = textOfParts(msg.parts);
    if (text && text.length >= out.text.length) {
      out.text = text;
      emitReply();
    }
    handleDataParts(msg.parts);
  };

  const handleTask = (task) => {
    if (!task) return;
    let text = "";
    for (const artifact of task.artifacts || []) {
      text += textOfParts(artifact.parts);
    }
    if (!text) text = textOfParts(task.status?.message?.parts);
    if (text && text.length >= out.text.length) {
      out.text = text;
      emitReply();
    }
    handleDataParts(task.status?.message?.parts);
  };

  const handleStatusUpdate = (su) => {
    const state = su.status?.state;
    const msg = su.status?.message;
    if (!msg || !msg.parts) return;
    const text = textOfParts(msg.parts);

    if (state === "TASK_STATE_COMPLETED") {
      // The completed status carries the full reply — prefer it over any
      // partial/replace-mode chunk accumulation.
      if (text && text.length >= out.text.length) {
        out.text = text;
        emitReply();
      }
    } else if (state === "TASK_STATE_WORKING") {
      // Most agents relay tool activity here as plain text rather than as
      // structured data parts — a real infra-agent stream carries exactly
      // `dns_lookup: example.com` / `ip_info: 104.20.23.154` this way, and
      // nothing else. Emit it ALWAYS: these frames interleave with reply
      // tokens (the first artifactUpdate arrives before the first tool call),
      // so gating on `!out.text` — as this branch used to — dropped every
      // tool line the agent reported and left the activity view empty.
      if (text) {
        const previousActivity = activitySeen;
        activitySeen = mergeProgress(previousActivity, text);
        const delta = activitySeen.startsWith(previousActivity)
          ? activitySeen.slice(previousActivity.length)
          : text;
        const line = delta.trim();
        if (line) handlers.onActivity?.(line);
      }
      // `progressText` is a different job: the reply fallback when a stream
      // ends without one. That one genuinely only applies before any reply.
      if (text && !out.text) {
        out.progressText = mergeProgress(out.progressText, text);
        handlers.onProgress?.(out.progressText);
      }
    } else if (state === "TASK_STATE_FAILED") {
      out.failed = true;
      out.errorMessage = text || "The agent reported a failure.";
      handlers.onError?.(out.errorMessage);
    }
    handleDataParts(msg.parts);
  };

  const handleArtifactUpdate = (au) => {
    const text = textOfParts(au.artifact?.parts);
    if (text) {
      if (au.append) out.text += text;
      else out.text = text;
      emitReply();
    }
  };

  const handleFrame = (evt) => {
    const statusUpdate = evt.statusUpdate || evt.result?.statusUpdate;
    const artifactUpdate = evt.artifactUpdate || evt.result?.artifactUpdate;
    const message = evt.message || evt.result?.message;
    const task = evt.task || evt.result?.task;

    if (statusUpdate) handleStatusUpdate(statusUpdate);
    if (artifactUpdate) handleArtifactUpdate(artifactUpdate);
    if (message && !statusUpdate && !artifactUpdate) handleMessage(message);
    if (task && !statusUpdate && !artifactUpdate) handleTask(task);

    if (evt.error && !out.failed) {
      out.failed = true;
      out.errorMessage = evt.error.message || "Stream error";
      handlers.onError?.(out.errorMessage);
    }
  };

  const handleLine = (line) => {
    // Spec-legal SSE allows both `data: {...}` and `data:{...}`.
    if (!line.startsWith("data:")) return;
    const raw = line.slice(5).trim();
    if (!raw || raw === "[DONE]") return;
    try {
      handleFrame(JSON.parse(raw));
    } catch (err) {
      console.debug("a2a-stream: unparseable frame skipped", err);
    }
  };

  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";

  while (true) {
    const { done, value } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    const lines = buffer.split("\n");
    buffer = lines.pop();
    for (const line of lines) handleLine(line);
  }
  buffer += decoder.decode();
  if (buffer) handleLine(buffer); // trailing frame without final newline

  if (!out.text && out.progressText && !out.failed) {
    // Stream ended without a final artifact/completed text — keep the last
    // progress text rather than discarding what the user already saw.
    out.text = out.progressText;
    emitReply();
  }
  return out;
}

/**
 * Batches repeated cumulative-text renders into animation frames so long
 * streams don't re-render markdown on every SSE event.
 */
export function frameRenderer(render) {
  let pending = null;
  let scheduled = false;
  return (text) => {
    pending = text;
    if (scheduled) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      if (pending != null) render(pending);
      pending = null;
    });
  };
}

/** True when the scroller is close enough to the bottom to keep following. */
export function nearBottom(el, slack = 80) {
  return el.scrollHeight - el.scrollTop - el.clientHeight < slack;
}
