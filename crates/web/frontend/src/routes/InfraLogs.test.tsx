import { screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import type { LogEntry } from "../lib/types";
import InfraLogs from "./InfraLogs";

const entry: LogEntry = {
  timestamp: "2026-07-09T10:00:00Z",
  message: "serving initial configuration",
  unit: "seedling-proxy",
  stream: "stdout",
  infra: "proxy",
};

describe("InfraLogs", () => {
  it("streams and renders proxy logs with the component label", async () => {
    const { streamLogs } = renderWithSession(<InfraLogs />, {
      route: "/infra/proxy/logs",
      path: "/infra/:component/logs",
      logEntries: [entry],
    });
    expect(await screen.findByText("Proxy (Caddy)")).toBeTruthy();
    expect(await screen.findByText("serving initial configuration")).toBeTruthy();
    expect(streamLogs).toHaveBeenCalledTimes(1);
    expect(streamLogs.mock.calls[0][0]).toEqual({
      infra: "proxy",
      follow: true,
      tail: 100,
    });
  });

  it("labels the resolver component and shows the empty state", async () => {
    const { streamLogs } = renderWithSession(<InfraLogs />, {
      route: "/infra/resolver/logs",
      path: "/infra/:component/logs",
    });
    expect(await screen.findByText("Resolver (CoreDNS)")).toBeTruthy();
    expect(await screen.findByText("No log entries.")).toBeTruthy();
    expect(streamLogs.mock.calls[0][0]).toEqual({
      infra: "resolver",
      follow: true,
      tail: 100,
    });
  });
});
