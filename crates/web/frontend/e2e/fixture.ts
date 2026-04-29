// End-to-end fixture: spawns seedlingd in --stub-backends mode and
// seedling-web pointed at it, returning a handle Playwright tests can use.
//
// The daemon is booted with in-memory fakes for the host-system backends
// (no podman, systemd, nftables, Caddy or NAT64), so this can run on any
// box without sudo. The OI server, DB, scheduler, event broker and web
// server all run for real, which is what we want to exercise from the UI.

import { ChildProcessWithoutNullStreams, spawn, spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import net from "node:net";

const __dirname = dirname(fileURLToPath(import.meta.url));

export interface StackHandle {
  /** HTTP base URL for the seedling-web server, e.g. http://127.0.0.1:54321 */
  baseUrl: string;
  /** Path to the temporary root directory holding daemon data + client key. */
  root: string;
  /** Stop both processes and remove the temporary directory. */
  stop: () => Promise<void>;
}

export interface StartOptions {
  /** Path to the workspace root (where target/debug lives). Defaults to four levels up. */
  workspaceRoot?: string;
  /** Suppress process stdout/stderr unless an env var requests verbose mode. */
  silent?: boolean;
}

const DEFAULT_WORKSPACE = resolve(__dirname, "..", "..", "..", "..");

async function getFreePort(): Promise<number> {
  return await new Promise((resolveFn, reject) => {
    const srv = net.createServer();
    srv.unref();
    srv.on("error", reject);
    srv.listen(0, "127.0.0.1", () => {
      const addr = srv.address();
      if (addr && typeof addr === "object") {
        const { port } = addr;
        srv.close(() => resolveFn(port));
      } else {
        srv.close();
        reject(new Error("failed to allocate port"));
      }
    });
  });
}

async function waitForHttp(url: string, timeoutMs = 30_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  let lastErr: unknown;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url, { redirect: "manual" });
      // Anything other than connection refused counts: the server is up.
      if (res.status > 0) {
        return;
      }
    } catch (e) {
      lastErr = e;
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`timed out waiting for ${url}: ${String(lastErr)}`);
}

function generateClientKey(ctlBin: string, stateDir: string): { keyPath: string; fingerprint: string } {
  // seedling-ctl writes its key at $XDG_STATE_HOME/seedling/client.key and
  // prints the fingerprint to stdout. No server connection is made.
  const result = spawnSync(ctlBin, ["client", "fingerprint"], {
    env: { ...process.env, XDG_STATE_HOME: stateDir },
    encoding: "utf8",
  });
  if (result.status !== 0) {
    throw new Error(
      `seedling-ctl client fingerprint failed (status ${result.status}): ${result.stderr}`,
    );
  }
  const fingerprint = result.stdout.trim().split(/\s+/)[0];
  if (!/^[0-9a-f]{64}$/i.test(fingerprint)) {
    throw new Error(`unexpected fingerprint output: ${JSON.stringify(result.stdout)}`);
  }
  return {
    keyPath: join(stateDir, "seedling", "client.key"),
    fingerprint,
  };
}

interface ProcWatch {
  /** Resolves when `pattern` first appears in stdout/stderr. */
  waitFor(pattern: RegExp, timeoutMs?: number): Promise<string>;
}

function pipeOutput(
  child: ChildProcessWithoutNullStreams,
  label: string,
  silent: boolean,
): ProcWatch {
  const verbose = !silent || process.env.SEEDLING_E2E_VERBOSE === "1";
  const buffer: string[] = [];
  const listeners: Array<(line: string) => void> = [];
  const onChunk = (chunk: Buffer, sink: NodeJS.WriteStream) => {
    const text = chunk.toString();
    if (verbose) sink.write(`[${label}] ${text}`);
    for (const line of text.split(/\r?\n/)) {
      if (!line) continue;
      buffer.push(line);
      for (const fn of listeners) fn(line);
    }
  };
  child.stdout.on("data", (c: Buffer) => onChunk(c, process.stdout));
  child.stderr.on("data", (c: Buffer) => onChunk(c, process.stderr));
  return {
    async waitFor(pattern, timeoutMs = 30_000) {
      for (const line of buffer) {
        if (pattern.test(line)) return line;
      }
      return await new Promise<string>((resolveFn, reject) => {
        const timer = setTimeout(() => {
          const idx = listeners.indexOf(listener);
          if (idx >= 0) listeners.splice(idx, 1);
          reject(new Error(`[${label}] timed out waiting for ${pattern}`));
        }, timeoutMs);
        const listener = (line: string) => {
          if (pattern.test(line)) {
            clearTimeout(timer);
            const idx = listeners.indexOf(listener);
            if (idx >= 0) listeners.splice(idx, 1);
            resolveFn(line);
          }
        };
        listeners.push(listener);
      });
    },
  };
}

async function killGracefully(proc: ChildProcessWithoutNullStreams): Promise<void> {
  if (proc.exitCode !== null) return;
  proc.kill("SIGTERM");
  await new Promise<void>((resolveFn) => {
    const timer = setTimeout(() => {
      if (proc.exitCode === null) proc.kill("SIGKILL");
    }, 3000);
    proc.once("exit", () => {
      clearTimeout(timer);
      resolveFn();
    });
  });
}

export async function startStack(opts: StartOptions = {}): Promise<StackHandle> {
  const workspaceRoot = opts.workspaceRoot ?? DEFAULT_WORKSPACE;
  const silent = opts.silent ?? true;

  const daemonBin = join(workspaceRoot, "target", "debug", "seedling");
  const webBin = join(workspaceRoot, "target", "debug", "seedling-web");
  const ctlBin = join(workspaceRoot, "target", "debug", "seedling-ctl");
  for (const bin of [daemonBin, webBin, ctlBin]) {
    if (!existsSync(bin)) {
      throw new Error(
        `missing binary ${bin} — run 'cargo build' (or 'just build') first`,
      );
    }
  }

  const root = mkdtempSync(join(tmpdir(), "seedling-e2e-"));
  const dataDir = join(root, "daemon");
  const stateDir = join(root, "state");
  mkdirSync(dataDir, { recursive: true });
  mkdirSync(join(stateDir, "seedling"), { recursive: true });

  const { keyPath, fingerprint } = generateClientKey(ctlBin, stateDir);
  writeFileSync(join(dataDir, "authorized_keys"), `${fingerprint} e2e\n`);

  const oiPort = await getFreePort();
  const httpPort = await getFreePort();
  const wtPort = await getFreePort();

  const daemon = spawn(
    daemonBin,
    [
      "--stub-backends",
      "--without-btrfs",
      "--data-dir",
      dataDir,
      "--listen",
      `127.0.0.1:${oiPort}`,
      "--audit-log",
      join(dataDir, "audit.log"),
    ],
    {
      env: {
        ...process.env,
        SEEDLING_LOG: process.env.SEEDLING_LOG ?? "seedling=info,warn",
      },
    },
  );
  const daemonWatch = pipeOutput(daemon, "daemon", silent);

  const cleanup = async () => {
    await killGracefully(web).catch(() => {});
    await killGracefully(daemon).catch(() => {});
    try {
      rmSync(root, { recursive: true, force: true });
    } catch {
      // best effort
    }
  };

  let web!: ChildProcessWithoutNullStreams;

  try {
    await daemonWatch.waitFor(/seedling ready/);

    web = spawn(
      webBin,
      [
        "--dev-no-auth",
        "--daemon-trust-any",
        "--http-port",
        String(httpPort),
        "--wt-port",
        String(wtPort),
        "--daemon-addr",
        `127.0.0.1:${oiPort}`,
        "--key-file",
        keyPath,
      ],
      {
        env: {
          ...process.env,
          SEEDLING_WEB_LOG: process.env.SEEDLING_WEB_LOG ?? "seedling_web=info,warn",
        },
      },
    );
    pipeOutput(web, "web", silent);

    const baseUrl = `http://127.0.0.1:${httpPort}`;
    await waitForHttp(`${baseUrl}/healthz`).catch(() => waitForHttp(`${baseUrl}/`));

    return {
      baseUrl,
      root,
      stop: cleanup,
    };
  } catch (e) {
    await cleanup();
    throw e;
  }
}
