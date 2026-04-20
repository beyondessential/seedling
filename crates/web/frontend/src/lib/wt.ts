import type { Actor, LogEntry, LogStreamParams, OiRequest, OiResult, SeedlingEvent, VolumeRef } from "./types";
import type { UniRouter } from "./uni-router";

export interface OpenShellParams {
  app: string;
  name: string;
  rows: number;
  cols: number;
  params?: Record<string, string>;
}

export interface OpenVolumeShellParams {
  volumes: VolumeRef[];
  rows: number;
  cols: number;
}

export interface OpenShellResult {
  sessionId: string;
  /** Write raw stdin bytes here. */
  writer: WritableStreamDefaultWriter<Uint8Array>;
  /** Resolves with the exit code when the shell exits. */
  exitCode: Promise<number>;
  /** Raw PTY output from the shell. */
  stdout: ReadableStream<Uint8Array>;
  /** Stderr output (empty in PTY mode, but present). */
  stderr: ReadableStream<Uint8Array>;
}

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

  // w[routes.logs]
  async streamLogs(
    params: LogStreamParams,
    onEntry: (entry: LogEntry) => void,
    signal: AbortSignal,
  ): Promise<void> {
    await this._streamLines("/logs/stream", params, signal, (line) => {
      try { onEntry(JSON.parse(line) as LogEntry); } catch { /* skip malformed */ }
    });
  }

  // w[routes.events]
  async subscribeEvents(
    onEvent: (event: SeedlingEvent) => void,
    signal: AbortSignal,
  ): Promise<void> {
    await this._streamLines("/events/subscribe", {}, signal, (line) => {
      try { onEvent(JSON.parse(line) as SeedlingEvent); } catch { /* skip malformed */ }
    });
  }

  // w[shells.wire]
  // w[shells.ui]
  async openShell(
    params: OpenShellParams,
    uniRouter: UniRouter,
  ): Promise<OpenShellResult> {
    const stream = await this.wt.createBidirectionalStream();
    const writer = stream.writable.getWriter();
    const reader = stream.readable.getReader();

    const req: OiRequest = { method: "/shells/start", actor: this.actor, params };
    const encoder = new TextEncoder();
    await writer.write(encoder.encode(JSON.stringify(req) + "\n"));

    // Read the handshake response line.
    const handshakeLine = await this._readOneLine(reader);
    const handshake = JSON.parse(handshakeLine) as Record<string, unknown>;
    if ("error" in handshake) {
      const err = handshake.error as Record<string, unknown>;
      reader.releaseLock();
      throw new Error(String(err.message ?? err.code ?? "shell open failed"));
    }

    const result = handshake.result as Record<string, unknown>;
    const sessionId = result.session_id as string;
    const stdoutId = BigInt(result.stdout_stream_id as number);
    const stderrId = BigInt(result.stderr_stream_id as number);

    // Register for the two uni streams before they can arrive.
    const [stdout, stderr] = await Promise.all([
      uniRouter.register(stdoutId),
      uniRouter.register(stderrId),
    ]);

    // The exit frame arrives as the last line on the bidi recv side.
    // r[impl shells.exit]
    const exitCode = new Promise<number>((resolve) => {
      void (async () => {
        try {
          const line = await this._readOneLine(reader);
          const frame = JSON.parse(line) as Record<string, unknown>;
          resolve(typeof frame.exit_code === "number" ? frame.exit_code : -1);
        } catch {
          resolve(-1);
        } finally {
          reader.releaseLock();
        }
      })();
    });

    return { sessionId, writer, exitCode, stdout, stderr };
  }

  // w[volumes.shell-ui]
  async openVolumeShell(
    params: OpenVolumeShellParams,
    uniRouter: UniRouter,
  ): Promise<OpenShellResult> {
    const stream = await this.wt.createBidirectionalStream();
    const writer = stream.writable.getWriter();
    const reader = stream.readable.getReader();

    const req: OiRequest = { method: "/volumes/shell", actor: this.actor, params };
    const encoder = new TextEncoder();
    await writer.write(encoder.encode(JSON.stringify(req) + "\n"));

    const handshakeLine = await this._readOneLine(reader);
    const handshake = JSON.parse(handshakeLine) as Record<string, unknown>;
    if ("error" in handshake) {
      const err = handshake.error as Record<string, unknown>;
      reader.releaseLock();
      throw new Error(String(err.message ?? err.code ?? "volume shell open failed"));
    }

    const result = handshake.result as Record<string, unknown>;
    const sessionId = result.session_id as string;
    const stdoutId = BigInt(result.stdout_stream_id as number);
    const stderrId = BigInt(result.stderr_stream_id as number);

    const [stdout, stderr] = await Promise.all([
      uniRouter.register(stdoutId),
      uniRouter.register(stderrId),
    ]);

    const exitCode = new Promise<number>((resolve) => {
      void (async () => {
        try {
          const line = await this._readOneLine(reader);
          const frame = JSON.parse(line) as Record<string, unknown>;
          resolve(typeof frame.exit_code === "number" ? frame.exit_code : -1);
        } catch {
          resolve(-1);
        } finally {
          reader.releaseLock();
        }
      })();
    });

    return { sessionId, writer, exitCode, stdout, stderr };
  }

  /** Read one newline-terminated line from an already-locked reader. */
  private async _readOneLine(
    reader: ReadableStreamDefaultReader<Uint8Array>,
  ): Promise<string> {
    const decoder = new TextDecoder();
    let buf = "";
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      buf += decoder.decode(value, { stream: true });
      const idx = buf.indexOf("\n");
      if (idx !== -1) {
        return buf.slice(0, idx).trim();
      }
    }
    return buf.trim();
  }

  private async _streamLines(
    method: string,
    params: unknown,
    signal: AbortSignal,
    onLine: (line: string) => void,
  ): Promise<void> {
    const stream = await this.wt.createBidirectionalStream();
    const writer = stream.writable.getWriter();
    const reader = stream.readable.getReader();

    const req: OiRequest = { method, actor: this.actor, params };
    const encoder = new TextEncoder();
    await writer.write(encoder.encode(JSON.stringify(req) + "\n"));

    const cleanup = () => {
      writer.close().catch(() => undefined);
      reader.cancel().catch(() => undefined);
    };
    signal.addEventListener("abort", cleanup, { once: true });

    const decoder = new TextDecoder();
    let buf = "";
    let firstLine = true;
    try {
      while (!signal.aborted) {
        const { done, value } = await reader.read();
        if (done) break;
        buf += decoder.decode(value, { stream: true });
        let idx: number;
        while ((idx = buf.indexOf("\n")) !== -1) {
          const line = buf.slice(0, idx).trim();
          buf = buf.slice(idx + 1);
          if (!line) continue;
          if (firstLine) {
            firstLine = false;
            const parsed = JSON.parse(line) as Record<string, unknown>;
            if ("error" in parsed)
              throw new Error(
                String((parsed.error as Record<string, unknown>).message ?? parsed.error),
              );
            continue;
          }
          onLine(line);
        }
      }
    } finally {
      signal.removeEventListener("abort", cleanup);
      cleanup();
    }
  }
}
