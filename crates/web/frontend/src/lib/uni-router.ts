/**
 * Routes incoming WT server-initiated unidirectional streams to registered
 * handlers by the 8-byte big-endian stream ID prefix written by the gateway.
 *
 * Usage:
 *   const router = new UniRouter();
 *   // In the session: router.startPump(wt);
 *   // In a shell component: const stream = await router.register(id);
 */

// w[shells.wire]
export class UniRouter {
  private waiting = new Map<
    bigint,
    (stream: ReadableStream<Uint8Array>) => void
  >();
  private parked = new Map<bigint, ReadableStream<Uint8Array>>();

  /**
   * Register interest in the uni stream with the given stream ID.
   *
   * If the stream already arrived (parked), resolves immediately.
   * Otherwise resolves when the gateway delivers it.
   */
  register(streamId: bigint): Promise<ReadableStream<Uint8Array>> {
    const parked = this.parked.get(streamId);
    if (parked !== undefined) {
      this.parked.delete(streamId);
      return Promise.resolve(parked);
    }
    return new Promise((resolve) => {
      this.waiting.set(streamId, resolve);
    });
  }

  private deliver(streamId: bigint, stream: ReadableStream<Uint8Array>): void {
    const handler = this.waiting.get(streamId);
    if (handler !== undefined) {
      this.waiting.delete(streamId);
      handler(stream);
    } else {
      this.parked.set(streamId, stream);
    }
  }

  /**
   * Start the background pump that reads incoming WT uni streams, reads the
   * 8-byte BE stream ID prefix, and routes the remainder to the registered
   * handler.
   *
   * Call once after the WebTransport session is established.
   */
  startPump(wt: WebTransport): void {
    void this.pump(wt);
  }

  private async pump(wt: WebTransport): Promise<void> {
    const reader = wt.incomingUnidirectionalStreams.getReader();
    try {
      for (;;) {
        const { done, value: stream } = await reader.read();
        if (done) break;
        void this.processIncoming(stream);
      }
    } catch {
      // WT session closed; nothing to do.
    } finally {
      reader.releaseLock();
    }
  }

  private async processIncoming(
    stream: ReadableStream<Uint8Array>,
  ): Promise<void> {
    // Read exactly 8 bytes for the stream ID prefix, handling chunking.
    const reader = stream.getReader();
    const prefix = new Uint8Array(8);
    let filled = 0;
    try {
      while (filled < 8) {
        const { done, value } = await reader.read();
        if (done) return; // stream ended before prefix was complete
        const needed = 8 - filled;
        const take = Math.min(needed, value.length);
        prefix.set(value.subarray(0, take), filled);
        filled += take;
        if (value.length > take) {
          // Leftover bytes belong to the payload. Re-prepend them.
          const leftover = value.subarray(take);
          reader.releaseLock();
          const streamId = new DataView(prefix.buffer).getBigUint64(0, false);
          const rejoined = prependChunk(stream, leftover);
          this.deliver(streamId, rejoined);
          return;
        }
      }
    } catch {
      reader.releaseLock();
      return;
    }

    reader.releaseLock();
    const streamId = new DataView(prefix.buffer).getBigUint64(0, false);
    this.deliver(streamId, stream);
  }
}

/**
 * Returns a new ReadableStream that yields `chunk` first, then all remaining
 * chunks from `stream`.  Used to re-prepend leftover bytes after the prefix
 * split.
 */
function prependChunk(
  stream: ReadableStream<Uint8Array>,
  chunk: Uint8Array,
): ReadableStream<Uint8Array> {
  const reader = stream.getReader();
  let sentChunk = false;
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      if (!sentChunk) {
        sentChunk = true;
        controller.enqueue(chunk);
        return;
      }
      const { done, value } = await reader.read();
      if (done) {
        controller.close();
        reader.releaseLock();
      } else {
        controller.enqueue(value);
      }
    },
    cancel(reason) {
      return reader.cancel(reason);
    },
  });
}
