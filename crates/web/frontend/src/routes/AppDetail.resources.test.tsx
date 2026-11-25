import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import {
  backupAction,
  baseFixtures,
  healthcheck,
  makeDetail,
  makeContainer,
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

  // spec: UI#operator-capabilities
  it("stages a param edit from the edit row and applies it", async () => {
    const { request } = mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("domain")).toBeTruthy();
    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[0]);
    const input = screen.getByDisplayValue("example.com");
    fireEvent.change(input, { target: { value: "shop.example.com" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await screen.findByText("1 change is waiting to be applied.")).toBeTruthy();
    expect(callsTo(request, "/apps/params/set")).toEqual([]);
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() =>
      expect(callsTo(request, "/apps/params/set")).toEqual([
        ["/apps/params/set", { app: "myapp", params: { domain: "shop.example.com" } }],
      ]),
    );
  });

  // spec: ACN#presentation-of-kinds
  it("shows a quantity's resolved amount in the edit row and holds back one its kind rejects", async () => {
    const memory = {
      name: "memory",
      value: "1Gi",
      is_set: true,
      secret: false,
      kind: "quantity",
      range: { min: 1, max: 2 ** 31 },
      options: null,
      required: false,
      description: null,
      default_value: null,
    };
    mount(baseFixtures(makeDetail({ params: [memory] })), "write");
    const row = (await screen.findByText("memory")).closest("tr") as HTMLElement;
    fireEvent.click(within(row).getByRole("button", { name: "Edit" }));
    const input = screen.getByDisplayValue("1Gi");
    fireEvent.change(input, { target: { value: "1.5Gi" } });
    expect(screen.getByText(`= ${(1_610_612_736).toLocaleString()}`)).toBeTruthy();
    expect(iconButton("Stage this change").disabled).toBe(false);
    fireEvent.change(input, { target: { value: "512m" } });
    expect(screen.getByText("m is milli; did you mean 512Mi?")).toBeTruthy();
    expect(iconButton("Stage this change").disabled).toBe(true);
    fireEvent.change(input, { target: { value: "" } });
    expect(iconButton("Stage this change").disabled).toBe(true);
    expect(iconButton("Plan this change").disabled).toBe(true);
  });

  // spec: UI#operator-capabilities
  it("does not stage a set secret left blank", async () => {
    mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("admin_password")).toBeTruthy();
    const row = screen.getByText("admin_password").closest("tr") as HTMLElement;
    fireEvent.click(within(row).getByRole("button", { name: "Edit" }));
    expect(iconButton("Stage this change").disabled).toBe(true);
    fireEvent.keyDown(screen.getByDisplayValue(""), { key: "Enter" });
    expect(screen.queryByText("1 change is waiting to be applied.")).toBeNull();
  });

  // spec: UI#operator-capabilities
  // spec: PRM#parameter-changes
  it("applies several staged edits in one change", async () => {
    const { request } = mount(
      { ...baseFixtures(makeDetail()), "/apps/params/set": { generation: 2 } },
      "dangerous",
    );
    expect(await screen.findByText("domain")).toBeTruthy();
    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[0]);
    fireEvent.change(screen.getByDisplayValue("example.com"), {
      target: { value: "shop.example.com" },
    });
    fireEvent.click(iconButton("Stage this change"));

    fireEvent.click(screen.getByRole("button", { name: "Set param" }));
    fireEvent.change(screen.getByPlaceholderText("param name"), {
      target: { value: "LOG_LEVEL" },
    });
    fireEvent.change(screen.getByPlaceholderText("value"), {
      target: { value: "debug" },
    });
    fireEvent.click(iconButton("Stage this change"));

    expect(await screen.findByText("2 changes are waiting to be applied together.")).toBeTruthy();
    expect(screen.getByText("LOG_LEVEL")).toBeTruthy();
    expect(callsTo(request, "/apps/params/set")).toEqual([]);

    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() =>
      expect(callsTo(request, "/apps/params/set")).toEqual([
        [
          "/apps/params/set",
          { app: "myapp", params: { domain: "shop.example.com", LOG_LEVEL: "debug" } },
        ],
      ]),
    );
    await waitFor(() =>
      expect(screen.queryByText(/waiting to be applied/)).toBeNull(),
    );
  });

  // spec: ACN#presentation-of-kinds
  it("edits a boolean or integer param with its own input", async () => {
    const typed = (name: string, kind: string, over: Record<string, unknown> = {}) => ({
      name,
      kind,
      value: null,
      is_set: false,
      secret: false,
      required: false,
      description: null,
      default_value: null,
      range: null,
      options: null,
      ...over,
    });
    const { request } = mount(
      baseFixtures(
        makeDetail({
          params: [
            typed("canary", "boolean"),
            typed("replicas", "integer", { range: { min: 1, max: 9 } }),
          ],
        }),
      ),
      "write",
    );
    expect(await screen.findByText("canary")).toBeTruthy();

    const canary = screen.getByText("canary").closest("tr") as HTMLElement;
    fireEvent.click(within(canary).getByRole("button", { name: "Set" }));
    expect(iconButton("Stage this change").disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "true" }));
    fireEvent.click(iconButton("Stage this change"));

    const replicas = screen.getByText("replicas").closest("tr") as HTMLElement;
    fireEvent.click(within(replicas).getByRole("button", { name: "Set" }));
    const input = screen.getByRole("spinbutton") as HTMLInputElement;
    fireEvent.change(input, { target: { value: "12" } });
    expect(iconButton("Stage this change").disabled).toBe(true);
    fireEvent.change(input, { target: { value: "3" } });
    fireEvent.click(iconButton("Stage this change"));

    fireEvent.click(await screen.findByRole("button", { name: "Apply" }));
    await waitFor(() =>
      expect(callsTo(request, "/apps/params/set")).toEqual([
        ["/apps/params/set", { app: "myapp", params: { canary: "true", replicas: "3" } }],
      ]),
    );
  });

  // spec: UI#operator-capabilities
  // spec: UI#guarded-controls
  it("stages the unset of an optional param with the other edits", async () => {
    const { request } = mount(baseFixtures(makeDetail()), "dangerous");
    expect(await screen.findByText("domain")).toBeTruthy();
    fireEvent.click(iconButton("Unset"));
    expect(await screen.findByText("1 change is waiting to be applied.")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() =>
      expect(callsTo(request, "/apps/params/set")).toEqual([
        ["/apps/params/set", { app: "myapp", params: { domain: null } }],
      ]),
    );
  });

  // spec: UI#operator-capabilities
  it("discards staged edits without sending them", async () => {
    const { request } = mount(baseFixtures(makeDetail()), "dangerous");
    expect(await screen.findByText("domain")).toBeTruthy();
    fireEvent.click(iconButton("Unset"));
    fireEvent.click(await screen.findByRole("button", { name: "Discard" }));
    expect(screen.queryByText(/waiting to be applied/)).toBeNull();
    expect(callsTo(request, "/apps/params/set")).toEqual([]);
  });

  // spec: UI#operator-capabilities
  it("never shows a staged secret value", async () => {
    mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("admin_password")).toBeTruthy();
    const edit = screen.getAllByRole("button", { name: "Edit" });
    fireEvent.click(edit[edit.length - 1]);
    const field = document.querySelector('input[type="password"]') as HTMLInputElement;
    fireEvent.change(field, { target: { value: "correct-horse-battery" } });
    fireEvent.click(iconButton("Stage this change"));
    expect(await screen.findByText("pending")).toBeTruthy();
    expect(screen.queryByText("correct-horse-battery")).toBeNull();
  });

  // spec: UI#operator-capabilities
  it("plans the staged edits together with the one being drafted", async () => {
    const { request } = mount(
      {
        ...baseFixtures(makeDetail()),
        "/apps/plan": {
          diff: [],
          gates_would_run: ["domain"],
          errors: [],
          rejections: [{ name: "domain", reason: "must be a hostname" }],
        },
      },
      "write",
    );
    expect(await screen.findByText("domain")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Set param" }));
    fireEvent.change(screen.getByPlaceholderText("param name"), { target: { value: "LOG_LEVEL" } });
    fireEvent.change(screen.getByPlaceholderText("value"), { target: { value: "debug" } });
    fireEvent.click(iconButton("Stage this change"));
    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[0]);
    fireEvent.change(screen.getByDisplayValue("example.com"), { target: { value: "x" } });
    fireEvent.click(screen.getByRole("button", { name: "Plan this change" }));
    expect(await screen.findByText(/must be a hostname/)).toBeTruthy();
    expect(screen.getByText("Gates that would run (1)")).toBeTruthy();
    expect(callsTo(request, "/apps/plan")).toEqual([
      ["/apps/plan", { app: "myapp", proposed_params: { LOG_LEVEL: "debug", domain: "x" } }],
    ]);
    expect(callsTo(request, "/apps/params/set")).toEqual([]);
  });

  // spec: UI#operator-capabilities
  it("plans a single draft without applying it", async () => {
    const { request } = mount(
      {
        ...baseFixtures(makeDetail()),
        "/apps/plan": { diff: [], gates_would_run: [], errors: [], rejections: [] },
      },
      "write",
    );
    expect(await screen.findByText("domain")).toBeTruthy();
    fireEvent.click(screen.getAllByRole("button", { name: "Edit" })[0]);
    fireEvent.change(screen.getByDisplayValue("example.com"), { target: { value: "x" } });
    fireEvent.click(screen.getByRole("button", { name: "Plan this change" }));
    await waitFor(() =>
      expect(callsTo(request, "/apps/plan")).toEqual([
        ["/apps/plan", { app: "myapp", proposed_params: { domain: "x" } }],
      ]),
    );
    expect(callsTo(request, "/apps/params/set")).toEqual([]);
  });
});

describe("AppDetail resource limits", () => {
  it("shows a container's memory limit as a quantity", async () => {
    const web = makeWebDeployment();
    if (web.def?.kind === "deployment") web.def.container = makeContainer({ memory: 512 * 2 ** 20 });
    mount(baseFixtures(makeDetail({ resources: [web] })));
    expect(await screen.findByText("mem: 512Mi")).toBeTruthy();
  });
});

describe("AppDetail restarts", () => {
  // spec: UI#restart-history
  it("shows the instance's restart count and links to its history", async () => {
    mount(
      baseFixtures(
        makeDetail({
          resources: [
            makeWebDeployment({
              instances: [
                {
                  id: "inst-web-0",
                  display_name: "web-0",
                  lifecycle: "ready",
                  restarts: {
                    recent: 3,
                    window_secs: 1800,
                    total: 7,
                    last_at: "2026-07-09T10:00:00Z",
                    last_exit_code: 137,
                    last_exit_kind: "exited",
                  },
                },
              ],
            }),
          ],
        }),
      ),
    );
    const chip = await screen.findByRole("link", { name: /3/ });
    expect(chip.getAttribute("href")).toBe("/restarts?instance=inst-web-0");
  });

  // spec: UI#restart-history
  it("shows no restart chip for an instance with no history", async () => {
    mount(baseFixtures(makeDetail()));
    expect(await screen.findByText("web-0")).toBeTruthy();
    expect(screen.queryByRole("link", { name: /restarts/ })).toBeNull();
  });
});

describe("AppDetail resources", () => {
  // spec: UI#operator-capabilities
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

  // spec: UI#operator-capabilities
  // The daemon reports `current` as null when it could not read the operator's
  // scaling decision. Read as a number, `null + 1` is 1, so scaling up a
  // deployment running five instances sent scale=1 and tore four down: the
  // harm the null exists to prevent, by another route.
  it("will not adjust a scale it was not given", async () => {
    const web = makeWebDeployment();
    const { request } = mount(
      baseFixtures(
        makeDetail({
          resources: [{ ...web, scale: { low: 1, high: 5, current: null } }],
        }),
      ),
      "write",
    );
    expect(await screen.findByText("unavailable")).toBeTruthy();
    expect(iconButton("Scale up").disabled).toBe(true);
    expect(iconButton("Scale down").disabled).toBe(true);
    fireEvent.click(iconButton("Scale up"));
    expect(callsTo(request, "/apps/scale")).toEqual([]);
  });

  // spec: UI#guarded-controls
  it("restarts and stops a deployment", async () => {
    const { request } = mount(baseFixtures(makeDetail()), "dangerous");
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

  // spec: UI#shells
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

  // spec: UI#app-status
  it("shows the healthcheck's on_failure and check state beside the lifecycle state", async () => {
    const dep = makeWebDeployment();
    if (dep.def?.kind === "deployment") dep.def.container.healthcheck = healthcheck;
    dep.instances = [{ ...dep.instances[0], health: "passing" }];
    mount(baseFixtures(makeDetail({ resources: [dep] })));
    const indicator = await screen.findByText("healthcheck: healthy · replace");
    const chip = indicator.closest(".MuiChip-root")!;
    expect(chip.className).toContain("MuiChip-colorSuccess");
    expect(chip.getAttribute("tabindex")).toBe("0");
    expect(chip.closest("td")!.textContent).toContain("ready");
    expect(screen.getByText("healthcheck (command), replace on failure")).toBeTruthy();
  });

  // spec: UI#app-status
  it("colours a failing check in the error colour and a starting check in the warning colour", async () => {
    const colourOf = async (health: "failing" | "starting") => {
      const dep = makeWebDeployment();
      if (dep.def?.kind === "deployment") dep.def.container.healthcheck = healthcheck;
      dep.instances = [{ ...dep.instances[0], health }];
      const view = mount(baseFixtures(makeDetail({ resources: [dep] })));
      const text = await screen.findByText(
        health === "failing"
          ? "healthcheck: unhealthy · replace"
          : "healthcheck: starting · replace",
      );
      const cls = text.closest(".MuiChip-root")!.className;
      view.unmount();
      return cls;
    };
    expect(await colourOf("failing")).toContain("MuiChip-colorError");
    expect(await colourOf("starting")).toContain("MuiChip-colorWarning");
  });

  // spec: UI#app-status
  it("reveals the kind, on_failure and a truncated command on hover", async () => {
    const dep = makeWebDeployment();
    if (dep.def?.kind === "deployment") {
      dep.def.container.healthcheck = {
        ...healthcheck,
        cmd: ["sh", "-c", "x".repeat(200)],
      };
    }
    dep.instances = [{ ...dep.instances[0], health: "passing" }];
    mount(baseFixtures(makeDetail({ resources: [dep] })));
    const chip = (await screen.findByText(/^healthcheck: healthy/)).closest(".MuiChip-root")!;
    fireEvent.mouseOver(chip);
    const tooltip = await screen.findByRole("tooltip");
    expect(tooltip.textContent).toContain("healthcheck (command)");
    expect(tooltip.textContent).toContain("on_failure: replace");
    expect(tooltip.textContent).toContain("…");
    expect(tooltip.textContent!.length).toBeLessThan(160);
  });

  // spec: UI#app-status
  it("shows a failing check from the instance's health, not from its faults alone", async () => {
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
    expect(await screen.findByText("healthcheck: unhealthy · replace")).toBeTruthy();
  });

  // spec: UI#priority
  it("shows a Deployment's declared priority beside its lifecycle state in the app priority colours", async () => {
    const dep = makeWebDeployment({ priority: "elevated" });
    const first = mount(baseFixtures(makeDetail({ resources: [dep] })));
    const mark = (await screen.findByText("elevated")).closest(".MuiChip-root")!;
    expect(mark.className).toContain("MuiChip-colorSuccess");
    expect(mark.closest("td")!.textContent).toContain("ready");
    expect(mark.closest("td")).toBe(screen.getByText("ready").closest("td"));

    first.unmount();

    mount(baseFixtures(makeDetail({ resources: [makeWebDeployment({ priority: "low" })] })));
    const lowMark = (await screen.findByText("low")).closest(".MuiChip-root")!;
    expect(lowMark.className).toContain("MuiChip-colorWarning");
  });

  // spec: UI#priority
  it("does not mark a Deployment declared at normal", async () => {
    mount(baseFixtures(makeDetail({ resources: [makeWebDeployment({ priority: "normal" })] })));
    await screen.findByText("web-0");
    expect(screen.queryByText("normal")).toBeNull();
  });
});

describe("AppDetail gate markers", () => {
  // spec: UI#operator-capabilities
  it("marks a resource a gate holds and one waiting on a gate", async () => {
    const [web, data] = makeDetail().resources;
    mount(
      baseFixtures(
        makeDetail({
          resources: [
            { ...web, held_by: ["migrate", "warm"] },
            {
              ...data,
              type: "job",
              name: "report",
              waiting_on: [
                { kind: "gate", name: "migrate" },
                { kind: "deployment", name: "web" },
              ],
            },
          ],
        }),
      ),
    );
    expect(await screen.findByText("held by migrate, warm")).toBeTruthy();
    expect(
      screen.getByText("waiting on migrate, deployment web"),
    ).toBeTruthy();
  });

  // spec: UI#operator-capabilities
  it("names a pending check as the gate's check, not the satisfied gate", async () => {
    const [web] = makeDetail().resources;
    mount(
      baseFixtures(
        makeDetail({
          resources: [
            { ...web, waiting_on: [{ kind: "check", name: "migrate" }] },
          ],
        }),
      ),
    );
    expect(await screen.findByText("waiting on migrate check")).toBeTruthy();
  });

  it("marks nothing on a resource neither held nor waiting", async () => {
    mount(baseFixtures(makeDetail()));
    expect(await screen.findByText("gen 4")).toBeTruthy();
    expect(screen.queryByText(/^held by/)).toBeNull();
    expect(screen.queryByText(/^waiting on/)).toBeNull();
  });
});

describe("AppDetail actions", () => {
  // spec: UI#operator-capabilities
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

  // spec: UI#shells
  it("opens a no-param shell action immediately", async () => {
    const { openShell } = mount(baseFixtures(makeDetail()), "write");
    expect(await screen.findByText("db-shell")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "shell" }));
    expect(openShell).toHaveBeenCalledWith("myapp", "db-shell", {});
  });

  // spec: UI#shells
  it("collects shell params in a dialog before opening the session", async () => {
    const detail = makeDetail({
      actions: [backupAction, reindexAction, psqlShellAction],
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
