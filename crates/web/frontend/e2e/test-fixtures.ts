// Shared Playwright test object: every spec gets a `stack` that points at a
// per-worker daemon + seedling-web pair. Worker-scoped so spec files in the
// same worker share one stack — startup is ~1s but doing it per-test would
// still bloat suite time as we add more specs.
//
// Tests in the same worker should be careful not to leak state into one
// another. If isolation matters, scope a test to its own worker via
// `test.describe.configure({ mode: "serial" })` and reset between.

import { test as base, expect } from "@playwright/test";

import { startStack, type StackHandle } from "./fixture";

type Fixtures = {
  stack: StackHandle;
};

export const test = base.extend<Record<string, never>, Fixtures>({
  stack: [
    async ({}, use) => {
      const handle = await startStack();
      await use(handle);
      await handle.stop();
    },
    { scope: "worker" },
  ],
});

export { expect };
