import type { Actor, OiRequest, OiResult } from "./types";

function hexToBuffer(hex: string): ArrayBuffer {
  const bytes = new Uint8Array(hex.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(hex.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes.buffer;
}

export async function openWebTransport(
  url: string,
  certHashes: string[],
): Promise<WebTransport> {
  const wt = new WebTransport(url, {
    serverCertificateHashes: certHashes.map((hash) => ({
      algorithm: "sha-256",
      value: hexToBuffer(hash),
    })),
  });
  await wt.ready;
  return wt;
}

export class WtClient {
  constructor(
    private readonly wt: WebTransport,
    private readonly actor: Actor,
  ) {}

  get closed(): Promise<unknown> {
    return this.wt.closed;
  }

  async request(method: string, params: unknown): Promise<OiResult> {
    const stream = await this.wt.createBidirectionalStream();
    const writer = stream.writable.getWriter();
    const reader = stream.readable.getReader();

    const req: OiRequest = { method, actor: this.actor, params };
    const encoder = new TextEncoder();
    await writer.write(encoder.encode(JSON.stringify(req) + "\n"));
    await writer.close();

    const decoder = new TextDecoder();
    let raw = "";
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      raw += decoder.decode(value, { stream: true });
    }

    const body = JSON.parse(raw.trim()) as Record<string, unknown>;
    if ("error" in body) {
      return {
        ok: false,
        error: body.error as { code: string; message: string },
      };
    }
    return { ok: true, value: body.result ?? body };
  }
}
