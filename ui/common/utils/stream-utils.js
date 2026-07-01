/**
 * Streaming helpers for consuming heterogeneous LLM/diff responses.
 *
 * A response may arrive as a `Response`, a `ReadableStream`, an async iterable,
 * a sync iterable, or a single value (string, bytes, or `{ delta | text |
 * content | markdown }` chunk objects). `consumeStream` normalizes all of these
 * into a single accumulated string while forwarding each decoded chunk to a
 * callback, and honours an `AbortSignal` for cancellation.
 */

/** Decode a single stream chunk of any supported shape into text. */
export function extractChunkText(chunk, decoder) {
  if (chunk == null) return "";
  if (typeof chunk === "string") return chunk;
  if (chunk instanceof Uint8Array) {
    return decoder.decode(chunk, { stream: true });
  }
  if (chunk instanceof ArrayBuffer) {
    return decoder.decode(new Uint8Array(chunk), { stream: true });
  }
  if (typeof chunk === "object") {
    if (Object.hasOwn(chunk, "delta") && chunk.delta != null) {
      return String(chunk.delta);
    }
    if (
      Object.hasOwn(chunk, "text") &&
      chunk.text != null &&
      typeof chunk.text !== "function"
    ) {
      return String(chunk.text);
    }
    if (Object.hasOwn(chunk, "content") && chunk.content != null) {
      return String(chunk.content);
    }
    if (Object.hasOwn(chunk, "markdown") && chunk.markdown != null) {
      return String(chunk.markdown);
    }
  }
  return String(chunk);
}

export function isReadableStream(value) {
  return value && typeof value.getReader === "function";
}

export function isAsyncIterable(value) {
  return (
    value != null &&
    typeof value !== "string" &&
    typeof value[Symbol.asyncIterator] === "function"
  );
}

export function isIterable(value) {
  return (
    value != null &&
    typeof value !== "string" &&
    typeof value[Symbol.iterator] === "function"
  );
}

/**
 * Consume any supported streaming response, returning the full accumulated text.
 * @param {*} result - Response | ReadableStream | async/sync iterable | value
 * @param {AbortSignal} signal - cancellation signal; aborting stops consumption
 * @param {(text: string) => void} onChunk - called with each decoded chunk
 * @returns {Promise<string>} the accumulated text
 */
export async function consumeStream(result, signal, onChunk) {
  if (signal.aborted) return "";

  if (result instanceof Response) {
    if (!result.ok) {
      const errText = await result.text().catch(() => "");
      throw new Error(errText || `Server error ${result.status}`);
    }
    result = result.body || (await result.text());
  }

  const decoder = new TextDecoder();
  let accumulated = "";
  const processChunk = (text) => {
    if (signal.aborted) return;
    accumulated += text;
    onChunk(text);
  };

  if (isReadableStream(result)) {
    const reader = result.getReader();
    const onAbort = () => reader.cancel().catch(() => {});
    signal.addEventListener("abort", onAbort, { once: true });
    try {
      while (true) {
        const { value, done } = await reader.read();
        if (done || signal.aborted) break;
        if (value != null) processChunk(extractChunkText(value, decoder));
      }
      if (!signal.aborted) {
        const trailing = decoder.decode();
        if (trailing) processChunk(trailing);
      }
    } finally {
      signal.removeEventListener("abort", onAbort);
      reader.releaseLock();
    }
  } else if (isAsyncIterable(result)) {
    const iterator = result[Symbol.asyncIterator]();
    const onAbort = () => iterator.return?.().catch(() => {});
    signal.addEventListener("abort", onAbort, { once: true });
    try {
      while (true) {
        const { value, done } = await iterator.next();
        if (done || signal.aborted) break;
        processChunk(extractChunkText(value, decoder));
      }
    } finally {
      signal.removeEventListener("abort", onAbort);
    }
  } else if (isIterable(result)) {
    for (const chunk of result) {
      if (signal.aborted) break;
      processChunk(extractChunkText(chunk, decoder));
    }
  } else if (result != null) {
    processChunk(extractChunkText(result, decoder));
  }

  return accumulated;
}
