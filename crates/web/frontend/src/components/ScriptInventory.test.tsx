import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { ResourceDef, TemplatePreview } from "../lib/types";
import { ScriptInventory } from "./ScriptInventory";

const emptyPreview: TemplatePreview = {
  resources: [],
  params: [],
  actions: [],
  script_error: null,
};

describe("ScriptInventory", () => {
  it("shows all sections as empty when nothing is declared", () => {
    render(<ScriptInventory preview={emptyPreview} />);
    expect(screen.getByText("Resources (0)")).toBeTruthy();
    expect(screen.getByText("Params (0)")).toBeTruthy();
    expect(screen.getByText("Actions (0)")).toBeTruthy();
    expect(screen.getAllByText("None declared.")).toHaveLength(3);
    expect(screen.queryByRole("table")).toBeNull();
  });

  it("shows the script error alert when present", () => {
    render(
      <ScriptInventory
        preview={{ ...emptyPreview, script_error: "parse error at line 7" }}
      />,
    );
    expect(screen.getByText("parse error at line 7")).toBeTruthy();
  });

  it("renders resources with derived summaries", () => {
    const preview: TemplatePreview = {
      ...emptyPreview,
      resources: [
        {
          name: "web",
          type: "container",
          scale: { low: 1, high: 3 },
          def: {
            kind: "deployment",
            container: { image: "nginx:1.27" },
          } as unknown as ResourceDef,
        },
        {
          name: "site",
          type: "route",
          def: {
            kind: "ingress",
            service: "web",
            hostname: "shop.example",
            port: 8080,
            tls: true,
            dtls: false,
            http_terminate: null,
            redirect: null,
          },
        },
      ],
    };
    render(<ScriptInventory preview={preview} />);
    expect(screen.getByText("Resources (2)")).toBeTruthy();
    expect(screen.getByText("web")).toBeTruthy();
    expect(screen.getByText("scale 1..3 · nginx:1.27")).toBeTruthy();
    expect(screen.getByText("shop.example · web:8080")).toBeTruthy();
  });

  it("renders params with secret flag, requiredness, and defaults", () => {
    const preview: TemplatePreview = {
      ...emptyPreview,
      params: [
        {
          name: "db_password",
          value: null,
          is_set: false,
          kind: "string",
          required: true,
          secret: true,
          default_value: null,
          description: null,
        },
        {
          name: "replicas",
          value: null,
          is_set: false,
          kind: "int",
          required: false,
          secret: false,
          default_value: "2",
          description: "How many web containers",
        },
      ],
    };
    render(<ScriptInventory preview={preview} />);
    expect(screen.getByText("Params (2)")).toBeTruthy();
    expect(screen.getByText("db_password")).toBeTruthy();
    expect(screen.getByText("secret")).toBeTruthy();
    expect(screen.getByText("yes")).toBeTruthy();
    expect(screen.getByText("no")).toBeTruthy();
    expect(screen.getByText("2")).toBeTruthy();
    expect(screen.getByText("How many web containers")).toBeTruthy();
  });

  it("renders actions with kind and description", () => {
    const preview: TemplatePreview = {
      ...emptyPreview,
      actions: [
        {
          name: "migrate",
          kind: "action",
          description: "Run DB migrations",
          params: {},
          schedules: [],
        },
        { name: "psql", kind: "shell", description: null, params: {}, schedules: [] },
      ],
    };
    render(<ScriptInventory preview={preview} />);
    expect(screen.getByText("Actions (2)")).toBeTruthy();
    expect(screen.getByText("migrate")).toBeTruthy();
    expect(screen.getByText("Run DB migrations")).toBeTruthy();
    expect(screen.getByText("psql")).toBeTruthy();
  });
});
