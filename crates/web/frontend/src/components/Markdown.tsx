import { Box, Link } from "@mui/material";
import MarkdownToJsx, { type MarkdownToJSX } from "markdown-to-jsx";
import type { ReactNode } from "react";

/// Renderer for markdown text set via `app.description(...)` and
/// `resource.description(...)`. Two modes:
///
/// - `inline` (default): block-level markdown is flattened — paragraphs
///   become spans, headings/lists/blockquotes/etc. degrade to plain text —
///   so the description fits on a single line alongside chips and other
///   inline UI. Inline emphasis (**bold**, *italics*, `code`, links) is
///   preserved.
/// - default block mode: paragraphs render as `<p>`, with normal markdown
///   block elements available. Used for the app-level description on the
///   detail page.
export function Markdown({
  text,
  inline = false,
}: {
  text: string;
  inline?: boolean;
}): ReactNode {
  if (inline) {
    const overrides: MarkdownToJSX.Overrides = {
      // Flatten paragraphs to spans so they don't introduce line breaks.
      p: { component: "span" },
      // Block-level elements that don't make sense on one line: degrade
      // each to a span so their text content survives but the structure
      // doesn't.
      h1: { component: "span" },
      h2: { component: "span" },
      h3: { component: "span" },
      h4: { component: "span" },
      h5: { component: "span" },
      h6: { component: "span" },
      ul: { component: "span" },
      ol: { component: "span" },
      li: { component: "span" },
      blockquote: { component: "span" },
      pre: { component: "span" },
      hr: { component: "span" },
      a: linkOverride,
    };
    return (
      <MarkdownToJsx
        options={{
          overrides,
          forceWrapper: true,
          wrapper: "span",
          disableParsingRawHTML: true,
        }}
      >
        {text}
      </MarkdownToJsx>
    );
  }
  const overrides: MarkdownToJSX.Overrides = { a: linkOverride };
  return (
    <Box
      sx={{
        "& p": { my: 0.5 },
        "& p:first-of-type": { mt: 0 },
        "& p:last-of-type": { mb: 0 },
        "& code": {
          fontFamily: "monospace",
          fontSize: "0.85em",
          backgroundColor: "action.hover",
          px: 0.5,
          borderRadius: 0.5,
        },
      }}
    >
      <MarkdownToJsx options={{ overrides, disableParsingRawHTML: true }}>
        {text}
      </MarkdownToJsx>
    </Box>
  );
}

const linkOverride: MarkdownToJSX.Override = {
  component: ({
    children,
    ...props
  }: {
    children?: ReactNode;
    href?: string;
  }) => (
    <Link {...props} target="_blank" rel="noreferrer">
      {children}
    </Link>
  ),
};
