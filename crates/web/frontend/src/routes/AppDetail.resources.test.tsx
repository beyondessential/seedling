import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import {
  backupAction,
  baseFixtures,
  healthcheck,
  installAction,
  makeDetail,
  makeFault,
  makeWebDeployment,
  psqlShellAction,
  reindexAction,
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

/** Inner button of an IconActionButton, addressed by its accessible name. */
function iconButton(label: string): HTMLButtonElement {
  return screen.getByRole("button", { name: label }) as HTMLButtonElement;
}

describe("AppDetail params", () => {
  it("renders values, masking secrets and marking defaults", async () => {
    mount(baseFixtures(makeDetail()));
    expect(await screen.findByText("domain")).toBeTruthy();
    expect(screen.getByText("example.com")).toBeTruthy();
    expect(screen.getByText("Public hostname")).toBeTruthy();
    // Secret param is set but its value is never shown.
    expect(screen.getByText("admin_password")).toBeTruthy();
    expect(screen.getByText("••••••••")).toBeTruthy();
    // Unset param falls back to its default, marked as such.
    expect(screen.getByText("4")).toBeTruthy();
    expect(screen.getByText("(default)")).toBeTruthy();
  });

  it("disables mutating controls in read mode", async () => {
    mount(baseFixtures(makeDetail()));
    expect(await screen.findByText("domain")).toBeTruthy();
    const setParam = screen.getByRole("button", {
      name: "Set param",
    }) as HTMLButtonElement;
    expect(setParam.disabled).toBe(true);
    expect(iconButton("Scale up").disabled).toBe(true);
    for (const run of screen.getAllByRole("button", { name: "Run" })) {
      expect((run as HTMLButtonElement).disabled).toBe(true);
    }
  });

  // w[verify routes.apps]
  it("sets a param from the edit row", async () => {
    const { request } = mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("domain")).toBeTruthy();
    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[0]);
    const input = screen.getByDisplayValue("example.com");
    fireEvent.change(input, { target: { value: "shop.example.com" } });
    fireEvent.keyDown(input, { key: "Enter" });
    await waitFor(() =>
      expect(callsTo(request, "/apps/params/set")).toEqual([
        ["/apps/params/set", { app: "myapp", name: "domain", value: "shop.example.com" }],
      ]),
    );
  });

  // w[verify routes.apps]
  it("unsets an optional param", async () => {
    const { request } = mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("domain")).toBeTruthy();
    fireEvent.click(iconButton("Unset"));
    await waitFor(() =>
      expect(callsTo(request, "/apps/params/unset")).toEqual([
        ["/apps/params/unset", { app: "myapp", name: "domain" }],
      ]),
    );
  });

  it("adds a new param through the add row", async () => {
    const { request } = mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("domain")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Set param" }));
    fireEvent.change(screen.getByPlaceholderText("param name"), {
      target: { value: "LOG_LEVEL" },
    });
    fireEvent.change(screen.getByPlaceholderText("value"), {
      target: { value: "debug" },
    });
    fireEvent.click(iconButton("Save"));
    await waitFor(() =>
      expect(callsTo(request, "/apps/params/set")).toEqual([
        ["/apps/params/set", { app: "myapp", name: "LOG_LEVEL", value: "debug" }],
      ]),
    );
  });
});

describe("AppDetail resources", () => {
  // w[verify routes.apps]
  it("scales a deployment up and down", async () => {
    const { request } = mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("web-0")).toBeTruthy();
    fireEvent.click(iconButton("Scale up"));
    await waitFor(() =>
      expect(callsTo(request, "/apps/scale")).toEqual([
        ["/apps/scale", { app: "myapp", deployment: "web", scale: 3 }],
      ]),
    );
    fireEvent.click(iconButton("Scale down"));
    await waitFor(() =>
      expect(callsTo(request, "/apps/scale").at(-1)).toEqual([
        "/apps/scale",
        { app: "myapp", deployment: "web", scale: 1 },
      ]),
    );
  });

  it("restarts and stops a deployment", async () => {
    const { request } = mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("web-0")).toBeTruthy();
    fireEvent.click(iconButton("Restart deployment"));
    await waitFor(() =>
      expect(callsTo(request, "/apps/restart")).toEqual([
        ["/apps/restart", { app: "myapp", deployment: "web" }],
      ]),
    );
    fireEvent.click(iconButton("Stop resource"));
    await waitFor(() =>
      expect(callsTo(request, "/apps/resource/stop")).toEqual([
        ["/apps/resource/stop", { app: "myapp", kind: "deployment", name: "web" }],
      ]),
    );
  });

  // w[verify volumes.shell-ui.read-only]
  it("opens a volume shell read-only in read mode and read-write in write mode", async () => {
    const first = mount(baseFixtures(makeDetail()));
    expect(await screen.findByText("web-0")).toBeTruthy();
    fireEvent.click(iconButton("Open shell (read-only)"));
    expect(first.openVolumeShell).toHaveBeenCalledWith(
      [{ kind: "app", app: "myapp", volume: "data" }],
      "myapp.data",
      { readOnly: true },
    );
    first.unmount();

    const second = mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("web-0")).toBeTruthy();
    fireEvent.click(iconButton("Open shell"));
    expect(second.openVolumeShell).toHaveBeenCalledWith(
      [{ kind: "app", app: "myapp", volume: "data" }],
      "myapp.data",
      { readOnly: false },
    );
  });

  // w[verify routes.apps.healthcheck-indicator]
  it("shows a healthy healthcheck indicator for a ready instance", async () => {
    const dep = makeWebDeployment();
    if (dep.def?.kind === "deployment") dep.def.container.healthcheck = healthcheck;
    mount(baseFixtures(makeDetail({ resources: [dep] })));
    expect(await screen.findByText("healthy")).toBeTruthy();
    // The declared healthcheck and its on_failure response are visible on the
    // resource definition chip.
    expect(
      screen.getByText("healthcheck (command), restart on failure"),
    ).toBeTruthy();
  });

  // w[verify routes.apps.healthcheck-indicator]
  it("shows an unhealthy indicator when a health_check_failed fault targets the instance", async () => {
    const dep = makeWebDeployment({
      faults: [
        makeFault({
          kind: "health_check_failed",
          instance_id: "inst-web-0",
          description: "healthcheck exited 1",
        }),
      ],
    });
    if (dep.def?.kind === "deployment") dep.def.container.healthcheck = healthcheck;
    mount(baseFixtures(makeDetail({ resources: [dep] })));
    expect(await screen.findByText("unhealthy")).toBeTruthy();
  });
});

describe("AppDetail actions", () => {
  // w[verify routes.apps]
  it("invokes a no-param action through the dialog", async () => {
    const { request } = mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("db-shell")).toBeTruthy();
    // First Run button belongs to the backup action row.
    fireEvent.click(screen.getAllByRole("button", { name: "Run" })[0]);
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Run: backup")).toBeTruthy();
    expect(within(dialog).getByText("No params required.")).toBeTruthy();
    fireEvent.click(within(dialog).getByRole("button", { name: "Run" }));
    await waitFor(() =>
      expect(callsTo(request, "/apps/action/invoke")).toEqual([
        ["/apps/action/invoke", { app: "myapp", name: "backup", params: {} }],
      ]),
    );
  });

  it("prefills action param defaults and sends edited values", async () => {
    const { request } = mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("reindex")).toBeTruthy();
    fireEvent.click(screen.getAllByRole("button", { name: "Run" })[1]);
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Run: reindex")).toBeTruthy();
    const input = within(dialog).getByDisplayValue("2");
    fireEvent.change(input, { target: { value: "5" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Run" }));
    await waitFor(() =>
      expect(callsTo(request, "/apps/action/invoke")).toEqual([
        ["/apps/action/invoke", { app: "myapp", name: "reindex", params: { depth: "5" } }],
      ]),
    );
  });

  // w[verify shells.ui]
  it("opens a no-param shell action immediately", async () => {
    const { openShell } = mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("db-shell")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "shell" }));
    expect(openShell).toHaveBeenCalledWith("myapp", "db-shell", {});
  });

  // w[verify shells.ui]
  it("collects shell params in a dialog before opening the session", async () => {
    const detail = makeDetail({
      actions: [installAction, backupAction, reindexAction, psqlShellAction],
    });
    const { openShell } = mount(baseFixtures(detail), "write");
    expect(await screen.findByText("psql")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "shell" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("Open shell: psql")).toBeTruthy();
    const input = within(dialog).getByDisplayValue("main");
    fireEvent.change(input, { target: { value: "analytics" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "shell" }));
    expect(openShell).toHaveBeenCalledWith("myapp", "psql", {
      database: "analytics",
    });
  });
});
