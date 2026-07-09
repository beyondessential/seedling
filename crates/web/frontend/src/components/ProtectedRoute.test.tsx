import { render, screen } from "@testing-library/react";
import { useContext } from "react";
import { MemoryRouter, Route, Routes } from "react-router-dom";
import { describe, expect, it, vi } from "vitest";
import type { Session } from "../lib/session";
import { ProtectedRoute } from "./ProtectedRoute";
import { SessionContext } from "./SessionProvider";

// The chrome components pull in data fetching, xterm, etc. — stub them so
// this test exercises only ProtectedRoute's gating logic.
vi.mock("./Navbar", () => ({
  Navbar: () => <div data-testid="navbar" />,
}));
vi.mock("./EventsSidebar", () => ({
  EventsSidebar: () => <div data-testid="events-sidebar" />,
}));
vi.mock("./ShellsSidebar", () => ({
  ShellsSidebar: () => <div data-testid="shells-sidebar" />,
}));

type Ctx = React.ContextType<typeof SessionContext>;

function baseCtx(): Ctx {
  // Start from the context's own defaults so the shape stays in sync.
  let defaults: Ctx | undefined;
  function Grab() {
    defaults = useContext(SessionContext);
    return null;
  }
  render(<Grab />);
  return { ...defaults! };
}

function renderProtected(overrides: Partial<Ctx>) {
  const ctx: Ctx = { ...baseCtx(), probing: false, ...overrides };
  return render(
    <MemoryRouter initialEntries={["/"]}>
      <SessionContext.Provider value={ctx}>
        <Routes>
          <Route element={<ProtectedRoute />}>
            <Route path="/" element={<div>secret content</div>} />
          </Route>
          <Route path="/login" element={<div>login page</div>} />
        </Routes>
      </SessionContext.Provider>
    </MemoryRouter>,
  );
}

const session = { token: "t", client: {} } as unknown as Session;

describe("ProtectedRoute", () => {
  it("shows a spinner while the session probe is in flight", () => {
    renderProtected({ probing: true, session: null });
    expect(screen.getByRole("progressbar")).toBeTruthy();
    expect(screen.queryByText("secret content")).toBeNull();
    expect(screen.queryByText("login page")).toBeNull();
  });

  it("redirects to /login when there is no session", () => {
    renderProtected({ session: null });
    expect(screen.getByText("login page")).toBeTruthy();
    expect(screen.queryByText("secret content")).toBeNull();
  });

  it("renders the outlet and navbar when a session exists", () => {
    renderProtected({ session });
    expect(screen.getByText("secret content")).toBeTruthy();
    expect(screen.getByTestId("navbar")).toBeTruthy();
    expect(screen.queryByTestId("events-sidebar")).toBeNull();
    expect(screen.queryByTestId("shells-sidebar")).toBeNull();
  });

  it("shows the events sidebar when open, and the shells sidebar when tabs exist", () => {
    renderProtected({
      session,
      sidebarOpen: true,
      shellTabs: [{ id: "s1" }] as unknown as Ctx["shellTabs"],
    });
    expect(screen.getByTestId("events-sidebar")).toBeTruthy();
    expect(screen.getByTestId("shells-sidebar")).toBeTruthy();
  });
});
