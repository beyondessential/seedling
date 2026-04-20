import type { Actor, ConnectRequest, ConnectResponse } from "./types";
import { WtClient, openWebTransport } from "./wt";

export interface Session {
  token: string;
  actor: Actor;
  client: WtClient;
  wt: WebTransport;
}

export async function connect(credential: ConnectRequest): Promise<Session> {
  const res = await fetch("/connect", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(credential),
  });

  if (res.status === 401) {
    throw new AuthRequired();
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
