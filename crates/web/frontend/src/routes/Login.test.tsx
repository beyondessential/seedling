// Login render + error paths. The success path is skipped: it hands a real
// WebTransport session to the provider, which jsdom cannot host.
import { fireEvent, render, screen } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { describe, expect, it, vi, type Mock } from "vitest";
import { AuthRequired, BackendUnreachable, connect } from "../lib/session";
import Login from "./Login";

vi.mock("../lib/session", () => {
  class AuthRequired extends Error {
    readonly credential: string;
    constructor(credential = "password") {
      super("authentication required");
      this.credential = credential;
    }
  }
  class BackendUnreachable extends Error {
    constructor(detail: string) {
      super(`backend unreachable: ${detail}`);
    }
  }
  return { AuthRequired, BackendUnreachable, connect: vi.fn() };
});

// The default SessionContext value (session: null, setSession: noop) is
// exactly what an unauthenticated Login render needs, so no provider here.
function renderLogin() {
  return render(
    <MemoryRouter initialEntries={["/login"]}>
      <Login />
    </MemoryRouter>,
  );
}

describe("Login", () => {
  it("renders the form with the submit disabled until a password is typed", () => {
    renderLogin();
    const button = screen.getByRole("button", { name: "Sign in" });
    expect(button.hasAttribute("disabled")).toBe(true);
    fireEvent.change(screen.getByLabelText(/Password/), {
      target: { value: "hunter2" },
    });
    expect(button.hasAttribute("disabled")).toBe(false);
  });

  it("shows an invalid-password error when connect rejects with AuthRequired", async () => {
    (connect as Mock).mockRejectedValue(new AuthRequired());
    renderLogin();
    fireEvent.change(screen.getByLabelText(/Password/), {
      target: { value: "wrong" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText("Invalid password.")).toBeTruthy();
    expect(connect).toHaveBeenCalledWith({ password: "wrong" });
  });

  it("shows the raw message for other connection failures", async () => {
    (connect as Mock).mockRejectedValue(new BackendUnreachable("fetch failed"));
    renderLogin();
    fireEvent.change(screen.getByLabelText(/Password/), {
      target: { value: "hunter2" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(
      await screen.findByText("backend unreachable: fetch failed"),
    ).toBeTruthy();
  });

  // spec: UI#signing-in
  it("shows that sign-in is unavailable, without a password prompt, when no credential would authenticate", async () => {
    (connect as Mock).mockRejectedValue(new AuthRequired("none"));
    renderLogin();
    fireEvent.change(screen.getByLabelText(/Password/), {
      target: { value: "anything" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Sign in" }));
    expect(await screen.findByText(/Sign-in is unavailable/)).toBeTruthy();
    expect(screen.queryByLabelText(/Password/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Sign in" })).toBeNull();
  });
});
