import { Stack, Tooltip, Typography } from "@mui/material";
import type { ImageSummary } from "../lib/types";

/**
 * Pick the most meaningful reference to use when addressing an image in an
 * API call (remove, inspect, etc.). Prefers a human-readable tag, falls
 * back to the image-manifest digest, then any digest, then the bare id.
 */
export function primaryReference(img: ImageSummary): string {
  if (img.tags.length > 0) return img.tags[0];
  // Prefer a "manifest" digest (this image's own content) over a
  // "manifest_list" digest (the multi-arch tag that pulled us).
  const manifest = img.digests.find((d) => d.kind === "manifest");
  if (manifest) return manifest.reference;
  if (img.digests.length > 0) return img.digests[0].reference;
  return img.image_id;
}

/**
 * Render the tag / digest references for an image in a single table cell.
 * Tags show in the default body font. Digests are always rendered smaller,
 * and annotated as either the image's own manifest or the manifest-list
 * digest it was resolved through.
 */
export function ImageReferencesCell({ image }: { image: ImageSummary }) {
  const hasReferences = image.tags.length > 0 || image.digests.length > 0;

  if (!hasReferences) {
    return (
      <Typography
        variant="caption"
        sx={{
          color: "text.secondary",
          fontFamily: "monospace",
        }}
      >
        (dangling) {image.image_id.slice(0, 19)}
      </Typography>
    );
  }

  return (
    <Stack spacing={0.5}>
      {image.tags.map((tag) => (
        <Typography
          key={tag}
          variant="body2"
          sx={{ fontFamily: "monospace" }}
        >
          {tag}
        </Typography>
      ))}
      {image.digests.map((d) => {
        const kindLabel =
          d.kind === "manifest"
            ? "manifest"
            : d.kind === "manifest_list"
              ? "manifest list"
              : "digest";
        const tooltip =
          d.kind === "manifest"
            ? "This image's own manifest digest."
            : d.kind === "manifest_list"
              ? "Digest of the multi-arch manifest list this image was pulled from."
              : "Digest reference (kind unknown).";
        return (
          <Tooltip key={d.reference} title={tooltip} placement="right">
            <Typography
              variant="caption"
              component="div"
              sx={{
                fontFamily: "monospace",
                color: "text.secondary",
                fontSize: "0.72rem",
                lineHeight: 1.35,
              }}
            >
              <Typography
                component="span"
                variant="caption"
                sx={{
                  fontSize: "0.68rem",
                  textTransform: "uppercase",
                  letterSpacing: "0.04em",
                  color:
                    d.kind === "manifest_list" ? "warning.main" : "text.disabled",
                  mr: 0.75,
                }}
              >
                {kindLabel}
              </Typography>
              {d.reference}
            </Typography>
          </Tooltip>
        );
      })}
    </Stack>
  );
}
