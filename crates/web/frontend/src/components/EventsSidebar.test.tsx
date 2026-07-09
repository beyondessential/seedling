import { fireEvent, screen, within } from "@testing-library/react";
import { useLocation } from "react-router-dom";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import type { SeedlingEvent } from "../lib/types";
import { EventsSidebar } from "./EventsSidebar";

function LocationProbe() {
  const location = useLocation();
  return <span data-testid="pathname">{location.pathname}</span>;
}

const events: SeedlingEvent[] = [
  {
    type: "FaultFiled",
    timestamp: "2026-07-09T10:00:00Z",
    app: "shop",
    kind: "container_crashed",
    description: "exited 137",
  },
  {
    type: "OperationCompleted",
    timestamp: "2026-07-09T09:59:00Z",
    app: "shop",
    action_name: "backup",
  },
];

// w[verify routes.events]
describe("EventsSidebar", () => {
  it("renders the empty state with infrastructure defaulting to stopped", async () => {
    renderWithSession(<EventsSidebar />);
    expect(await screen.findByText("No events yet.")).toBeTruthy();
    expect(screen.getByText("Infrastructure")).toBeTruthy();
    expect(screen.getByText("Proxy")).toBeTruthy();
    expect(screen.getByText("Resolver")).toBeTruthy();
    expect(screen.getAllByText("stopped")).toHaveLength(2);
  });

  it("renders event rows with type chip, summary, and app link", async () => {
    renderWithSession(<EventsSidebar />, { events });
    expect(await screen.findByText("Fault Filed")).toBeTruthy();
    expect(screen.getByText("fault: container_crashed — exited 137")).toBeTruthy();
    expect(screen.getByText("Operation Completed")).toBeTruthy();
    expect(screen.getByText("backup completed")).toBeTruthy();
    const appLinks = screen.getAllByRole("link", { name: "shop" });
    expect(appLinks).toHaveLength(2);
    expect(appLinks[0].getAttribute("href")).toBe("/apps/shop");
    // Cached-event counter in the header.
    expect(screen.getByText("2")).toBeTruthy();
  });

  it("shows infra component status from /infra/status", async () => {
    renderWithSession(<EventsSidebar />, {
      fixtures: { "/infra/status": { proxy: "running", resolver: "stopped" } },
    });
    expect(await screen.findByText("running")).toBeTruthy();
    expect(screen.getByText("stopped")).toBeTruthy();
  });

  it("navigates to the infra log view when the logs button is clicked", async () => {
    renderWithSession(
      <>
        <EventsSidebar />
        <LocationProbe />
      </>,
    );
    const proxyRow = (await screen.findByText("Proxy")).parentElement!;
    fireEvent.click(within(proxyRow).getByRole("button"));
    expect(screen.getByTestId("pathname").textContent).toBe("/infra/proxy/logs");
  });
});
