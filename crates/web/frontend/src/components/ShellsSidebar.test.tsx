import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SessionContext, type ShellTab } from "./SessionProvider";
import { ShellsSidebar } from "./ShellsSidebar";

vi.mock("@xterm/xterm", () => ({
  Terminal: class {
    rows = 24;
    cols = 80;
    loadAddon() {}
    open() {}
    onResize() {}
    onData() {}
    write() {}
    dispose() {}
  },
}));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/xterm/css/xterm.css", () => ({}));

class FakeResizeObserver {
  observe() {}
  disconnect() {}
}

type Ctx = React.ContextType<typeof SessionContext>;

function setup(tabs: ShellTab[]) {
  let resolveExit: (code: number) => void = () => undefined;
  const openVolumeShell = vi.fn(
    async (_params: { read_only?: boolean }) =>
      ({
        sessionId: "sess",
        writer: { write: async () => undefined, close: async () => undefined },
        exitCode: new Promise<number>((r) => {
          resolveExit = r;
        }),
        stdout: new ReadableStream<Uint8Array>(),
        stderr: new ReadableStream<Uint8Array>(),
      }) as never,
  );
  const request = vi.fn(async () => ({ ok: true as const, value: null }));
  const closeShell = vi.fn();
  const ctx = {
    session: { client: { openVolumeShell, request } },
    uniRouter: {},
    shellTabs: tabs,
    activeShellId: tabs[0].id,
    setActiveShellId: vi.fn(),
    closeShell,
    shellsSidebarWidth: 600,
    setShellsSidebarWidth: vi.fn(),
  } as unknown as Ctx;
  render(
    <SessionContext.Provider value={ctx}>
      <ShellsSidebar />
    </SessionContext.Provider>,
  );
  return { openVolumeShell, request, closeShell, exit: (c: number) => resolveExit(c) };
}

const roTab: ShellTab = {
  kind: "volume",
  id: "1",
  volumes: [{ kind: "site", name: "data" }],
  label: "data",
  readOnly: true,
};
const rwTab: ShellTab = { ...roTab, id: "2", label: "shop.uploads", readOnly: false };

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", FakeResizeObserver);
});

describe("ShellsSidebar", () => {
  // spec: UI#shells
  it("has one tab per shell, marking a read-only volume shell's label", () => {
    setup([roTab, rwTab]);
    expect(screen.getAllByRole("tab")).toHaveLength(2);
    expect(screen.getByText("data (ro)")).toBeTruthy();
    expect(screen.getByText("shop.uploads")).toBeTruthy();
  });

  // spec: UI#shells
  it("opens each shell with the mounts its tab was opened under", async () => {
    const { openVolumeShell } = setup([roTab, rwTab]);
    await waitFor(() => expect(openVolumeShell).toHaveBeenCalledTimes(2));
    const readOnly = openVolumeShell.mock.calls.map((c) => c[0].read_only);
    expect(readOnly).toEqual([true, false]);
  });

  // spec: UI#shells
  it("shows the exit code and reopens an exited shell, or closes it", async () => {
    const { openVolumeShell, closeShell, exit } = setup([roTab]);
    await waitFor(() => expect(openVolumeShell).toHaveBeenCalledTimes(1));
    await act(async () => exit(3));
    expect(await screen.findByText("Exited: 3")).toBeTruthy();

    fireEvent.click(screen.getByRole("button", { name: "Reopen" }));
    await waitFor(() => expect(openVolumeShell).toHaveBeenCalledTimes(2));
    expect(openVolumeShell.mock.calls[1][0].read_only).toBe(true);
    expect(screen.queryByText("Exited: 3")).toBeNull();

    await act(async () => exit(0));
    fireEvent.click(await screen.findByRole("button", { name: "Close" }));
    expect(closeShell).toHaveBeenCalledWith("1");
  });
});
