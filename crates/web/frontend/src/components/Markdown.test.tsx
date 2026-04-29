import { render } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { Markdown } from "./Markdown";

describe("Markdown", () => {
  it("renders inline emphasis without introducing block elements (inline mode)", () => {
    const { container } = render(
      <Markdown text="Use **bold** and *italic* and `code`" inline />,
    );
    const html = container.innerHTML;
    expect(html).toContain("<strong>bold</strong>");
    expect(html).toContain("<em>italic</em>");
    expect(html).toContain("<code>code</code>");
    // No paragraph breaks: a single span wrapper holds the content.
    expect(container.querySelectorAll("p").length).toBe(0);
  });

  it("flattens paragraph breaks to spans in inline mode", () => {
    const { container } = render(
      <Markdown text={"first paragraph\n\nsecond paragraph"} inline />,
    );
    expect(container.querySelectorAll("p").length).toBe(0);
    expect(container.textContent).toContain("first paragraph");
    expect(container.textContent).toContain("second paragraph");
  });

  it("flattens markdown lists in inline mode (no <ul>/<li>)", () => {
    const { container } = render(
      <Markdown text={"- one\n- two\n- three"} inline />,
    );
    expect(container.querySelectorAll("ul").length).toBe(0);
    expect(container.querySelectorAll("li").length).toBe(0);
    expect(container.textContent).toContain("one");
    expect(container.textContent).toContain("two");
    expect(container.textContent).toContain("three");
  });

  it("renders multiple paragraphs as <p> in block mode", () => {
    const { container } = render(
      <Markdown text={"first paragraph\n\nsecond paragraph"} />,
    );
    expect(container.querySelectorAll("p").length).toBe(2);
  });

  it("does not parse raw HTML (so <script> can't sneak in)", () => {
    const { container } = render(
      <Markdown text={'<script>alert("xss")</script>'} inline />,
    );
    expect(container.querySelector("script")).toBeNull();
  });
});
