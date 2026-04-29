// daemon.connect-retry: when seedling-web can't reach the daemon at
// startup, it must keep retrying with exponential backoff rather than
// exiting. We verify by pointing it at a port nothing is listening on
// and watching the backoff schedule walk through 1s → 2s.
//
// w[verify daemon.connect-retry]

import { expect, test } from "@playwright/test";

import { freePort, spawnWebPointedAt } from "../fixture";

test("seedling-web retries the daemon connection with backoff instead of exiting", async () => {
  const deadDaemonPort = await freePort();
  const handle = await spawnWebPointedAt(`127.0.0.1:${deadDaemonPort}`);
  try {
    // First retry: warning log includes "retrying in 1s".
    await handle.watch.waitFor(/daemon connection failed.*retrying in 1s/i, 15_000);
    expect(handle.proc.exitCode).toBeNull();

    // Second retry doubles the backoff to 2s. Seeing it land confirms the
    // loop is actually iterating, not silently stuck.
    await handle.watch.waitFor(/daemon connection failed.*retrying in 2s/i, 10_000);
    expect(handle.proc.exitCode).toBeNull();
  } finally {
    await handle.stop();
  }
});
