import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import type { ImageSummary } from "../lib/types";
import { ImageReferencesCell, primaryReference } from "./ImageReferences";

function image(overrides: Partial<ImageSummary>): ImageSummary {
  return {
    image_id: "sha256:0123456789abcdef0123456789abcdef",
    tags: [],
    digests: [],
    manifest_digest: null,
    size_bytes: 1024,
    created_at: "2026-07-01T00:00:00Z",
    last_used_at: "2026-07-08T00:00:00Z",
    in_use: false,
    pinned_by: [],
    ...overrides,
  };
}

describe("primaryReference", () => {
  it("prefers the first tag", () => {
    const img = image({
      tags: ["nginx:1.27", "nginx:latest"],
      digests: [{ reference: "reg/img@sha256:aaa", kind: "manifest" }],
    });
    expect(primaryReference(img)).toBe("nginx:1.27");
  });

  it("falls back to the manifest digest over a manifest-list digest", () => {
    const img = image({
      digests: [
        { reference: "reg/img@sha256:list", kind: "manifest_list" },
        { reference: "reg/img@sha256:own", kind: "manifest" },
      ],
    });
    expect(primaryReference(img)).toBe("reg/img@sha256:own");
  });

  it("falls back to any digest, then the bare image id", () => {
    const withDigest = image({
      digests: [{ reference: "reg/img@sha256:list", kind: "manifest_list" }],
    });
    expect(primaryReference(withDigest)).toBe("reg/img@sha256:list");
    expect(primaryReference(image({}))).toBe(
      "sha256:0123456789abcdef0123456789abcdef",
    );
  });
});

describe("ImageReferencesCell", () => {
  it("shows a truncated dangling marker when there are no references", () => {
    render(<ImageReferencesCell image={image({})} />);
    // image_id truncated to its first 19 characters
    expect(screen.getByText("(dangling) sha256:0123456789ab")).toBeTruthy();
  });

  it("lists tags and annotated digests", () => {
    render(
      <ImageReferencesCell
        image={image({
          tags: ["nginx:1.27"],
          digests: [
            { reference: "reg/img@sha256:own", kind: "manifest" },
            { reference: "reg/img@sha256:list", kind: "manifest_list" },
            { reference: "reg/img@sha256:mystery", kind: "unknown" },
          ],
        })}
      />,
    );
    expect(screen.getByText("nginx:1.27")).toBeTruthy();
    expect(screen.getByText("manifest")).toBeTruthy();
    expect(screen.getByText("manifest list")).toBeTruthy();
    expect(screen.getByText("digest")).toBeTruthy();
    expect(screen.getByText(/reg\/img@sha256:own/)).toBeTruthy();
    expect(screen.getByText(/reg\/img@sha256:list/)).toBeTruthy();
    expect(screen.getByText(/reg\/img@sha256:mystery/)).toBeTruthy();
  });
});
