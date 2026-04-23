import { describe, expect, it } from "vitest";
import { UniRouter } from "./uni-router";

/**
 * Stubs the `WebTransport` surface the router uses: only
 * `incomingUnidirectionalStreams`. Tests construct a ReadableStream of
 * ReadableStream<Uint8Array>, each framed with an 8-byte BE stream ID, and
 * feed it through a fake transport.
 */
function fakeTransport(
  uniStreams: ReadableStream<Uint8Array>[],
): WebTransport {
  const queue = [...uniStreams];
  const incoming = new ReadableStream<ReadableStream<Uint8Array>>({
    pull(controller) {
      const next = queue.shift();
      if (next === undefined) controller.close();
      else controller.enqueue(next);
    },
  });
  return { incomingUnidirectionalStreams: incoming } as unknown as WebTransport;
}

function streamWith(prefixId: bigint, payload: Uint8Array): ReadableStream<Uint8Array> {
  const prefix = new Uint8Array(8);
  new DataView(prefix.buffer).setBigUint64(0, prefixId, false);
  const combined = new Uint8Array(prefix.length + payload.length);
  combined.set(prefix, 0);
  combined.set(payload, prefix.length);
  return new ReadableStream<Uint8Array>({
    start(c) {
      c.enqueue(combined);
      c.close();
    },
  });
}

function streamInChunks(prefixId: bigint, payload: Uint8Array, splitAt: number): ReadableStream<Uint8Array> {
  const prefix = new Uint8Array(8);
  new DataView(prefix.buffer).setBigUint64(0, prefixId, false);
  const combined = new Uint8Array(prefix.length + payload.length);
  combined.set(prefix, 0);
  combined.set(payload, prefix.length);
  const first = combined.slice(0, splitAt);
  const second = combined.slice(splitAt);
  return new ReadableStream<Uint8Array>({
    start(c) {
      c.enqueue(first);
      c.enqueue(second);
      c.close();
    },
  });
}

async function collect(stream: ReadableStream<Uint8Array>): Promise<number[]> {
  const chunks: Uint8Array[] = [];
  const reader = stream.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
  }
  reader.releaseLock();
  const out: number[] = [];
  for (const c of chunks) {
    for (const b of c) out.push(b);
  }
  return out;
}

function toArray(u: Uint8Array): number[] {
  const out: number[] = [];
  for (const b of u) out.push(b);
  return out;
}

// w[verify shells.wire]
describe("UniRouter", () => {
  it("routes a stream to a handler registered before delivery", async () => {
    const router = new UniRouter();
    const payload = new TextEncoder().encode("hello");
    const wt = fakeTransport([streamWith(7n, payload)]);

    const incoming = router.register(7n);
    router.startPump(wt);

    const stream = await incoming;
    expect(await collect(stream)).toEqual(toArray(payload));
  });

  it("parks a stream that arrives before its handler registers", async () => {
    const router = new UniRouter();
    const payload = new TextEncoder().encode("parked");
    const wt = fakeTransport([streamWith(42n, payload)]);

    router.startPump(wt);
    // Give the pump a tick to ingest and park the stream.
    await new Promise<void>((resolve) => setTimeout(resolve, 10));

    const stream = await router.register(42n);
    expect(await collect(stream)).toEqual(toArray(payload));
  });

  it("handles the prefix arriving split across chunks", async () => {
    const router = new UniRouter();
    const payload = new TextEncoder().encode("chunked");
    // Split so the first chunk only contains 3 of the 8 prefix bytes.
    const wt = fakeTransport([streamInChunks(99n, payload, 3)]);

    const incoming = router.register(99n);
    router.startPump(wt);

    const stream = await incoming;
    expect(await collect(stream)).toEqual(toArray(payload));
  });

  it("preserves payload leftover when the first chunk also carries some payload", async () => {
    const router = new UniRouter();
    const payload = new TextEncoder().encode("leftover-bytes");
    // Split the combined 8+payload at 12 — first chunk has the whole prefix
    // plus 4 payload bytes, remainder goes in a second chunk.
    const wt = fakeTransport([streamInChunks(5n, payload, 12)]);

    const incoming = router.register(5n);
    router.startPump(wt);

    const stream = await incoming;
    expect(await collect(stream)).toEqual(toArray(payload));
  });

  it("routes multiple interleaved streams to their handlers", async () => {
    const router = new UniRouter();
    const a = new TextEncoder().encode("alpha");
    const b = new TextEncoder().encode("bravo");
    const wt = fakeTransport([streamWith(1n, a), streamWith(2n, b)]);

    const pA = router.register(1n);
    const pB = router.register(2n);
    router.startPump(wt);

    expect(await collect(await pA)).toEqual(toArray(a));
    expect(await collect(await pB)).toEqual(toArray(b));
  });
});
