import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import {
  baseFixtures,
  makeDetail,
  makeFault,
} from "./AppDetail.fixtures";
import AppDetail from "./AppDetail";

const ROUTE = { route: "/apps/myapp", path: "/apps/:name" };

function mount(
  fixtures: Record<string, unknown>,
  safetyMode?: "read" | "write" | "dangerous",
) {
  return renderWithSession(<AppDetail />, { ...ROUTE, fixtures, safetyMode });
}

function callsTo(request: ReturnType<typeof mount>["request"], method: string) {
  return request.mock.calls.filter((c) => c[0] === method);
}

describe("AppDetail page", () => {
  it("shows a spinner while the app query is loading", async () => {
    mount(baseFixtures(makeDetail()));
    expect(screen.getByRole("progressbar")).toBeTruthy();
    // Let the fixture resolve so effects settle.
    expect(await screen.findByText("gen 4")).toBeTruthy();
  });

  it("shows an error alert when the app query fails", async () => {
    mount({
      "/apps/show": {
        ok: false,
        error: { code: "not_found", message: "no such app" },
      },
    });
    expect(await screen.findByText(/no such app/)).toBeTruthy();
  });

  // spec: UI#operator-capabilities
  it("renders header, status, params, actions, schedules, and resources", async () => {
    mount(baseFixtures(makeDetail()));
    expect(await screen.findByText("gen 4")).toBeTruthy();
    // Status chip and markdown description.
    expect(screen.getByText("running")).toBeTruthy();
    expect(screen.getByText("testing")).toBeTruthy();
    // Section headings.
    for (const heading of ["Params", "Actions", "Schedules", "Resources", "Images"]) {
      expect(screen.getByText(heading)).toBeTruthy();
    }
    // Actions table: install action row is hidden, others visible.
    expect(screen.queryByText("on_install")).toBeNull();
    expect(screen.getAllByText("backup").length).toBeGreaterThan(0);
    expect(screen.getByText("db-shell")).toBeTruthy();
    expect(screen.getByText("scheduled")).toBeTruthy();
    // Schedules table shows the cron expression.
    expect(screen.getByText("0 3 * * *")).toBeTruthy();
    // Resources: deployment with scale bounds and instance state.
    expect(screen.getByText("web")).toBeTruthy();
    expect(screen.getByText("web-0")).toBeTruthy();
    expect(screen.getByText("[1–5]")).toBeTruthy();
    expect(screen.getByText("ready")).toBeTruthy();
    // Installed app offers Uninstall, not Deregister.
    expect(screen.getByRole("button", { name: "Uninstall" })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Deregister" })).toBeNull();
  });

  it("renders faults and clears them through the confirm dialog", async () => {
    const fault = makeFault();
    const { request } = mount(
      baseFixtures(makeDetail({ faults: [fault] })),
      "write",
    );
    expect(await screen.findByText("Faults")).toBeTruthy();
    expect(screen.getAllByText("container_crashed").length).toBeGreaterThan(0);
    // spec: UI#guarded-controls
    // Clearing faults needs write, even for a running app.
    fireEvent.click(screen.getByRole("button", { name: "Clear all" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Clear faults" }));
    await waitFor(() =>
      expect(callsTo(request, "/faults/clear")).toEqual([
        ["/faults/clear", { app: "myapp" }],
      ]),
    );
  });

  it("shows the in-flight operation, gates actions, and can cancel it", async () => {
    const detail = makeDetail({
      status: "operating",
      current_operation: {
        action_name: "backup",
        source_generation: 4,
        target_generation: 5,
        barrier: {
          resources: ["web"],
          required_state: "ready",
          deadline_secs: 120,
          elapsed_secs: 33.4,
        },
      },
    });
    const { request } = mount(baseFixtures(detail), "dangerous");
    expect(await screen.findByText(/Operation in progress/)).toBeTruthy();
    expect(screen.getByText(/barrier: ready \(33s \/ 120s\)/)).toBeTruthy();
    // Params stay editable during an operation: a change does not wait for it.
    expect(
      (screen.getByRole("button", { name: "Set param" }) as HTMLButtonElement).disabled,
    ).toBe(false);
    // The running action shows a progress row; the other action can't run.
    expect(screen.getByRole("button", { name: /Running…/ })).toBeTruthy();
    const run = screen.getByRole("button", { name: "Run" }) as HTMLButtonElement;
    expect(run.disabled).toBe(true);
    // spec: ACI#cancellation
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await waitFor(() =>
      expect(callsTo(request, "/apps/action/cancel")).toEqual([
        ["/apps/action/cancel", { app: "myapp" }],
      ]),
    );
  });

  it("surfaces stopped resources and unstops them", async () => {
    const detail = makeDetail({
      stopped_resources: [{ kind: "deployment", name: "web" }],
      resources: [
        { ...makeDetail().resources[0], stopped: true },
        makeDetail().resources[1],
      ],
    });
    const { request } = mount(baseFixtures(detail), "write");
    expect(await screen.findByText(/Partially running/)).toBeTruthy();
    expect(screen.getByText("stopped")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Unstop all" }));
    await waitFor(() =>
      expect(callsTo(request, "/apps/unstop")).toEqual([
        ["/apps/unstop", { app: "myapp" }],
      ]),
    );
    // Per-resource unstop on the stopped deployment row.
    const unstop = screen.getByRole("button", { name: "Unstop resource" });
    fireEvent.click(unstop);
    await waitFor(() =>
      expect(callsTo(request, "/apps/resource/unstop")).toEqual([
        ["/apps/resource/unstop", { app: "myapp", kind: "deployment", name: "web" }],
      ]),
    );
  });

  // spec: UI#guarded-controls
  it("requires the dangerous mode to stop a resource, and only write to unstop one", async () => {
    mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("gen 4")).toBeTruthy();
    const stop = screen.getByRole("button", { name: "Stop resource" }) as HTMLButtonElement;
    expect(stop.disabled).toBe(true);
  });

  it("allows stopping a resource in the dangerous mode", async () => {
    mount(baseFixtures(makeDetail()), "dangerous");
    expect(await screen.findByText("gen 4")).toBeTruthy();
    const stop = screen.getByRole("button", { name: "Stop resource" }) as HTMLButtonElement;
    expect(stop.disabled).toBe(false);
  });

  it("uninstalls an installed app through the confirm dialog", async () => {
    const { request } = mount(baseFixtures(makeDetail()), "dangerous");
    expect(await screen.findByText("gen 4")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Uninstall" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Uninstall app")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Uninstall" }));
    await waitFor(() =>
      expect(callsTo(request, "/apps/uninstall")).toEqual([
        ["/apps/uninstall", { app: "myapp" }],
      ]),
    );
  });

  describe("not-installed app", () => {
    const notInstalled = () =>
      makeDetail({
        status: "not_installed",
        resources: [],
        params: [],
        actions: [],
      });

    it("offers install directly, with only the app", async () => {
      const { request } = mount(baseFixtures(notInstalled()), "write");
      expect(
        await screen.findByText("This app has not been installed yet."),
      ).toBeTruthy();
      expect(screen.getByRole("button", { name: "Deregister" })).toBeTruthy();
      expect(screen.queryByRole("button", { name: "Uninstall" })).toBeNull();
      fireEvent.click(screen.getByRole("button", { name: "Install" }));
      expect(screen.queryByRole("dialog")).toBeNull();
      await waitFor(() =>
        expect(callsTo(request, "/apps/install")).toEqual([
          ["/apps/install", { app: "myapp" }],
        ]),
      );
    });

    it("deregisters the app through the confirm dialog", async () => {
      const { request } = mount(baseFixtures(notInstalled()), "dangerous");
      fireEvent.click(await screen.findByRole("button", { name: "Deregister" }));
      const dialog = await screen.findByRole("dialog");
      expect(within(dialog).getByText(/cannot be undone/)).toBeTruthy();
      fireEvent.click(within(dialog).getByRole("button", { name: "Deregister" }));
      await waitFor(() =>
        expect(callsTo(request, "/apps/remove")).toEqual([
          ["/apps/remove", { app: "myapp" }],
        ]),
      );
    });
  });

  it("lists external volumes with their mappings", async () => {
    const detail = makeDetail({
      resources: [
        ...makeDetail().resources,
        {
          name: "shared",
          type: "external_volume",
          instances: [],
          faults: [],
          def: { kind: "external_volume" },
        },
        {
          name: "scratch",
          type: "external_volume",
          instances: [],
          faults: [],
          def: { kind: "external_volume" },
        },
      ],
    });
    const fixtures = {
      ...baseFixtures(detail),
      "/volumes/external/list": [
        {
          app: "myapp",
          external_name: "shared",
          read_only: true,
          target: { kind: "site", name: "bigdisk" },
        },
      ],
    };
    mount(fixtures);
    expect(await screen.findByText("External Volumes")).toBeTruthy();
    expect(await screen.findByText("bigdisk")).toBeTruthy();
    expect(screen.getByText("ro")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Remap" })).toBeTruthy();
    // The unmapped volume offers Map instead.
    expect(screen.getByText("Not mapped")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Map" })).toBeTruthy();
  });
});
