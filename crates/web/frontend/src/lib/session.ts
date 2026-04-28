import type { Actor, ConnectRequest, ConnectResponse } from "./types";
import { WtClient, openWebTransport } from "./wt";

export interface Session {
  token: string;
  actor: Actor;
  client: WtClient;
  wt: WebTransport;
}

export async function connect(credential: ConnectRequest): Promise<Session> {
  let res: Response;
  try {
    res = await fetch("/connect", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(credential),
    });
  } catch (e) {
    // Network-level failure (DNS, refused, TLS, abort) — the backend
    // isn't reachable, which is categorically different from "you need
    // to log in".
    throw new BackendUnreachable(e instanceof Error ? e.message : String(e));
  }

  if (res.status === 401) {
    throw new AuthRequired();
  }
  if (res.status >= 500) {
    throw new BackendUnreachable(`POST /connect returned ${res.status}`);
  }
  if (!res.ok) {
    throw new Error(`POST /connect failed: ${res.status}`);
  }

  const data = (await res.json()) as ConnectResponse;
  const wt = await openWebTransport(data.wt_url, data.cert_hashes);
  const client = new WtClient(wt, data.actor);
  return { token: data.token, actor: data.actor, client, wt };
}

export class AuthRequired extends Error {
  constructor() {
    super("authentication required");
  }
}

export class BackendUnreachable extends Error {
  constructor(detail: string) {
    super(`backend unreachable: ${detail}`);
  }
}
