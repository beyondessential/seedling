// Vitest setup: Node >= 25 defines experimental `localStorage`/`sessionStorage`
// getters on globalThis that shadow jsdom's storage (and return undefined
// without --localstorage-file), because the jsdom test environment aliases
// `window` to `globalThis`. Install a plain in-memory Web Storage
// implementation so both globals behave identically everywhere.
class MemoryStorage implements Storage {
  private store = new Map<string, string>();

  get length(): number {
    return this.store.size;
  }

  clear(): void {
    this.store.clear();
  }

  getItem(key: string): string | null {
    return this.store.has(key) ? (this.store.get(key) ?? null) : null;
  }

  key(index: number): string | null {
    return [...this.store.keys()][index] ?? null;
  }

  removeItem(key: string): void {
    this.store.delete(key);
  }

  setItem(key: string, value: string): void {
    this.store.set(key, String(value));
  }
}

for (const name of ["localStorage", "sessionStorage"] as const) {
  Object.defineProperty(globalThis, name, {
    value: new MemoryStorage(),
    writable: true,
    configurable: true,
  });
}
