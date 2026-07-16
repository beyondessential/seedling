import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { renderWithSession } from "../test/harness";
import type { CanopyStatus } from "../lib/types";
import Canopy from "./Canopy";

const notEnrolled: CanopyStatus = { enrolled: false };

const enrolled: CanopyStatus = {
  enrolled: true,
  server_id: "srv-0195b2",
  device_id: "dev-7f3a91",
  api_url: "https://canopy.example/api",
  last_push_at: "2026-07-06T10:15:00Z",
  last_response: { accepted: true },
};

// w[verify routes.canopy]
describe("Canopy", () => {
  it("shows the enrol form with a masked passphrase when not enrolled", async () => {
    renderWithSession(<Canopy />, {
      fixtures: { "/canopy/status": notEnrolled },
    });
    expect(await screen.findByText(/not enrolled with Canopy/)).toBeTruthy();
    expect(screen.getByLabelText("Enrolment ticket")).toBeTruthy();
    const passphrase = screen.getByLabelText("Passphrase");
    expect(passphrase.getAttribute("type")).toBe("password");
    expect(screen.getByRole("button", { name: "Enrol" })).toBeTruthy();
  });

  it("shows registration details and hides the enrol form when enrolled", async () => {
    renderWithSession(<Canopy />, { fixtures: { "/canopy/status": enrolled } });
    expect(await screen.findByText("srv-0195b2")).toBeTruthy();
    expect(screen.getByText("dev-7f3a91")).toBeTruthy();
    expect(screen.getByText("https://canopy.example/api")).toBeTruthy();
    expect(
      screen.getByText(new Date(enrolled.last_push_at!).toLocaleString()),
    ).toBeTruthy();
    expect(screen.queryByLabelText("Enrolment ticket")).toBeNull();
    expect(screen.queryByLabelText("Passphrase")).toBeNull();
  });

  it("shows the last report error when the last report failed", async () => {
    renderWithSession(<Canopy />, {
      fixtures: {
        "/canopy/status": {
          ...enrolled,
          last_push_error: "connection refused",
        },
      },
    });
    expect(await screen.findByText("connection refused")).toBeTruthy();
  });

  it("enrols via /canopy/enrol and clears the ticket and passphrase", async () => {
    const { request } = renderWithSession(<Canopy />, {
      fixtures: {
        "/canopy/status": notEnrolled,
        "/canopy/enrol": { server_id: "srv-0195b2", device_id: "dev-7f3a91" },
      },
      safetyMode: "dangerous",
    });
    const ticket = await screen.findByLabelText("Enrolment ticket");
    fireEvent.change(ticket, { target: { value: "dGhpcyBpcyBhIHRpY2tldA==" } });
    fireEvent.change(screen.getByLabelText("Passphrase"), {
      target: { value: "correct horse battery" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Enrol" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual([
        "/canopy/enrol",
        { ticket: "dGhpcyBpcyBhIHRpY2tldA==", passphrase: "correct horse battery" },
      ]),
    );
    await waitFor(() => {
      expect(
        (screen.getByLabelText("Enrolment ticket") as HTMLTextAreaElement).value,
      ).toBe("");
      expect(
        (screen.getByLabelText("Passphrase") as HTMLInputElement).value,
      ).toBe("");
    });
  });

  it("clears the ticket and passphrase even when enrolment fails", async () => {
    renderWithSession(<Canopy />, {
      fixtures: {
        "/canopy/status": notEnrolled,
        "/canopy/enrol": {
          ok: false,
          error: { code: "requirements_invalid", message: "bad passphrase" },
        },
      },
      safetyMode: "dangerous",
    });
    const ticket = await screen.findByLabelText("Enrolment ticket");
    fireEvent.change(ticket, { target: { value: "dGlja2V0" } });
    fireEvent.change(screen.getByLabelText("Passphrase"), {
      target: { value: "nope" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Enrol" }));
    expect(await screen.findByText(/bad passphrase/)).toBeTruthy();
    expect(
      (screen.getByLabelText("Enrolment ticket") as HTMLTextAreaElement).value,
    ).toBe("");
    expect((screen.getByLabelText("Passphrase") as HTMLInputElement).value).toBe(
      "",
    );
  });

  it("shows an error alert when the status query fails", async () => {
    renderWithSession(<Canopy />, {
      fixtures: {
        "/canopy/status": {
          ok: false,
          error: { code: "internal", message: "canopy status exploded" },
        },
      },
    });
    expect(await screen.findByText(/canopy status exploded/)).toBeTruthy();
  });

  it("deregisters via /canopy/deregister only after confirmation", async () => {
    const { request } = renderWithSession(<Canopy />, {
      fixtures: {
        "/canopy/status": enrolled,
        "/canopy/deregister": { deregistered: true },
      },
      safetyMode: "dangerous",
    });
    fireEvent.click(await screen.findByRole("button", { name: "Deregister" }));
    // Opening the confirmation dialog must not itself issue the request.
    expect(
      request.mock.calls.some(([method]) => method === "/canopy/deregister"),
    ).toBe(false);
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Deregister" }));
    await waitFor(() =>
      expect(request.mock.calls).toContainEqual(["/canopy/deregister", {}]),
    );
  });

  it("keeps enrol disabled in read mode", async () => {
    renderWithSession(<Canopy />, {
      fixtures: { "/canopy/status": notEnrolled },
    });
    const ticket = await screen.findByLabelText("Enrolment ticket");
    fireEvent.change(ticket, { target: { value: "dGlja2V0" } });
    fireEvent.change(screen.getByLabelText("Passphrase"), {
      target: { value: "pass" },
    });
    expect(screen.getByRole("button", { name: "Enrol" })).toHaveProperty(
      "disabled",
      true,
    );
  });
});
