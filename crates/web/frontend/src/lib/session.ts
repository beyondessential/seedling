import type { Actor, ConnectRequest, ConnectResponse } from "./types";
import { WtClient, openWebTransport } from "./wt";

export interface Session {
  token: string;
  actor: Actor;
  client: WtClient;
  wt: WebTransport;
}

let heldToken: string | null = null;

/** The body `/connect` is sent: at most one of `token` and `password`, with a
 *  held session token standing in for a credential the caller supplies none of. */
// spec: WEB#authentication
export function connectBody(credential: ConnectRequest, held: string | null): ConnectRequest {
  if (credential.password !== undefined) return { password: credential.password };
  const token = credential.token ?? held;
  return token === null ? {} : { token };
}

export async function connect(credential: ConnectRequest): Promise<Session> {
  const body = connectBody(credential, heldToken);
  let res: Response;
  try {
    res = await fetch("/connect", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch (e) {
    // Network-level failure (DNS, refused, TLS, abort) — the backend
    // isn't reachable, which is categorically different from "you need
    // to log in".
    throw new BackendUnreachable(e instanceof Error ? e.message : String(e));
  }

  if (res.status === 401) {
    if (body.token !== undefined) heldToken = null;
    throw new AuthRequired(await authRequiredCredential(res));
  }
  if (res.status >= 500) {
    throw new BackendUnreachable(`POST /connect returned ${res.status}`);
  }
  if (!res.ok) {
    throw new Error(`POST /connect failed: ${res.status}`);
  }

  const data = (await res.json()) as ConnectResponse;
  heldToken = data.token;
  const wt = await openWebTransport(data.wt_url, data.cert_hashes);
  const client = new WtClient(wt, data.actor);
  return { token: data.token, actor: data.actor, client, wt };
}

/** What the operator must supply for the next `/connect`: a password, or
 *  nothing, because no credential would authenticate. */
export type AuthCredential = "password" | "none";

// spec: UI#signing-in
async function authRequiredCredential(res: Response): Promise<AuthCredential> {
  try {
    const body = (await res.json()) as { auth_required?: unknown };
    return body.auth_required === "none" ? "none" : "password";
  } catch {
    return "password";
  }
}

export class AuthRequired extends Error {
  readonly credential: AuthCredential;

  constructor(credential: AuthCredential = "password") {
    super("authentication required");
    this.credential = credential;
  }
}

/** Whether a heartbeat answer says the web session is no longer recorded,
 *  so the client must close it and open a new one. */
// spec: WEB#session-tracking
export function sessionDropped(heartbeat: unknown): boolean {
  return (
    typeof heartbeat === "object" &&
    heartbeat !== null &&
    (heartbeat as { alive?: unknown }).alive === false
  );
}

/** Forget the held session token, as when the operator signs out. */
export function forgetSessionToken() {
  heldToken = null;
}

export class BackendUnreachable extends Error {
  constructor(detail: string) {
    super(`backend unreachable: ${detail}`);
  }
}
