import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it } from "vitest";
import { SessionContext } from "../components/SessionProvider";
import { makeFakeClient, renderWithSession } from "../test/harness";
import type { Session } from "../lib/session";
import type { LogEntry } from "../lib/types";
import Logs from "./Logs";

function entry(overrides: Partial<LogEntry> = {}): LogEntry {
  return {
    timestamp: "2026-07-09T10:00:00Z",
    message: "listening on :8080",
    unit: "app-shop-web",
    stream: "stdout",
    app: "shop",
    ...overrides,
  };
}

// w[verify routes.logs]
describe("Logs", () => {
  it("streams and renders log lines for the app scope", async () => {
    const { streamLogs } = renderWithSession(<Logs />, {
      route: "/apps/shop/logs",
      path: "/apps/:name/logs",
      logEntries: [
        entry(),
        entry({ message: "GET / 200", instance: "0123abcd" }),
      ],
    });
    expect(await screen.findByText("listening on :8080")).toBeTruthy();
    expect(screen.getByText("GET / 200")).toBeTruthy();
    // Instance shown when the scope is not a single instance.
    expect(screen.getByText("0123abcd")).toBeTruthy();
    expect(screen.getByText("2 lines")).toBeTruthy();
    expect(screen.getByText("(all)")).toBeTruthy();
    expect(streamLogs).toHaveBeenCalledTimes(1);
    expect(streamLogs.mock.calls[0][0]).toEqual({
      app: "shop",
      resource: undefined,
      instance: undefined,
      follow: true,
      tail: 100,
    });
  });

  it("marks stderr lines with an err tag", async () => {
    renderWithSession(<Logs />, {
      route: "/apps/shop/logs",
      path: "/apps/:name/logs",
      logEntries: [entry({ message: "oh no", stream: "stderr" })],
    });
    expect(await screen.findByText("oh no")).toBeTruthy();
    expect(screen.getByText("err")).toBeTruthy();
  });

  it("scopes the stream by resource and instance from query params", async () => {
    const { streamLogs } = renderWithSession(<Logs />, {
      route: "/apps/shop/logs?resource=web&instance=0123abcd",
      path: "/apps/:name/logs",
    });
    expect(await screen.findByText("web / 0123abcd")).toBeTruthy();
    expect(streamLogs.mock.calls[0][0]).toEqual({
      app: "shop",
      resource: "web",
      instance: "0123abcd",
      follow: true,
      tail: 100,
    });
  });

  it("shows the empty state when the stream ends without entries", async () => {
    renderWithSession(<Logs />, {
      route: "/apps/shop/logs",
      path: "/apps/:name/logs",
    });
    expect(await screen.findByText("No log entries.")).toBeTruthy();
    expect(screen.getByText("0 lines")).toBeTruthy();
  });

  it("restarts the stream with the new tail when the selector changes", async () => {
    const { streamLogs } = renderWithSession(<Logs />, {
      route: "/apps/shop/logs",
      path: "/apps/:name/logs",
      logEntries: [entry()],
    });
    await screen.findByText("listening on :8080");
    fireEvent.mouseDown(screen.getByRole("combobox"));
    fireEvent.click(await screen.findByRole("option", { name: "500" }));
    await waitFor(() => expect(streamLogs).toHaveBeenCalledTimes(2));
    expect(streamLogs.mock.calls[1][0]).toEqual({
      app: "shop",
      resource: undefined,
      instance: undefined,
      follow: true,
      tail: 500,
    });
  });

  it("shows an error alert when the stream fails", async () => {
    const { client, streamLogs } = makeFakeClient();
    streamLogs.mockRejectedValue(new Error("stream reset by peer"));
    const session = {
      token: "test-token",
      actor: { kind: "test", id: "test" },
      client,
      wt: { close: () => undefined },
    } as unknown as Session;
    const ctx = { session, events: [] } as unknown as React.ContextType<
      typeof SessionContext
    >;
    render(
      <MemoryRouter initialEntries={["/apps/shop/logs"]}>
        <SessionContext.Provider value={ctx}>
          <Routes>
            <Route path="/apps/:name/logs" element={<Logs />} />
          </Routes>
        </SessionContext.Provider>
      </MemoryRouter>,
    );
    expect(await screen.findByText("stream reset by peer")).toBeTruthy();
  });
});
