import { render, screen } from "@testing-library/react";
import { createMemoryRouter, RouterProvider } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import ErrorPage from "./ErrorPage";

function Boom(): never {
  throw new Error("kaboom from render");
}

describe("ErrorPage", () => {
  beforeEach(() => {
    // React logs caught render errors; keep test output clean.
    vi.spyOn(console, "error").mockImplementation(() => undefined);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows the message of a thrown Error", async () => {
    const router = createMemoryRouter([
      { path: "/", element: <Boom />, errorElement: <ErrorPage /> },
    ]);
    render(<RouterProvider router={router} />);
    expect(await screen.findByText("Something went wrong")).toBeTruthy();
    expect(screen.getByText("kaboom from render")).toBeTruthy();
  });

  it("falls back to statusText for router error responses", async () => {
    const router = createMemoryRouter([
      {
        path: "/",
        loader: () => {
          throw new Response("", { status: 404, statusText: "Not Found" });
        },
        element: <div>never shown</div>,
        errorElement: <ErrorPage />,
      },
    ]);
    render(<RouterProvider router={router} />);
    expect(await screen.findByText("Something went wrong")).toBeTruthy();
    expect(screen.getByText("Not Found")).toBeTruthy();
  });
});
