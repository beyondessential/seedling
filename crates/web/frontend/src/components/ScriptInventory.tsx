import {
  Alert,
  Box,
  Chip,
  Paper,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  Typography,
} from "@mui/material";
import type { TemplatePreview, TemplatePreviewResource } from "../lib/types";

interface Props {
  preview: TemplatePreview;
}

export function ScriptInventory({ preview }: Props) {
  return (
    <Stack spacing={2}>
      {preview.script_error && (
        <Alert severity="error" sx={{ fontFamily: "monospace" }}>
          {preview.script_error}
        </Alert>
      )}

      <Section title="Resources" count={preview.resources.length}>
        {preview.resources.length > 0 && (
          <TableContainer component={Paper} variant="outlined">
            <Table size="small">
              <TableHead>
                <TableRow>
                  <TableCell>Name</TableCell>
                  <TableCell>Type</TableCell>
                  <TableCell>Summary</TableCell>
                </TableRow>
              </TableHead>
              <TableBody>
                {preview.resources.map((r) => (
                  <TableRow key={`${r.type}/${r.name}`}>
                    <TableCell sx={{ fontFamily: "monospace" }}>{r.name}</TableCell>
                    <TableCell>
                      <Chip label={r.type} size="small" variant="outlined" />
                    </TableCell>
                    <TableCell
                      sx={{
                        fontFamily: "monospace",
                        color: "text.secondary",
                        fontSize: "0.75rem",
                      }}
                    >
                      {resourceSummary(r)}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </TableContainer>
        )}
      </Section>

      <Section title="Params" count={preview.params.length}>
        {preview.params.length > 0 && (
          <TableContainer component={Paper} variant="outlined">
            <Table size="small">
              <TableHead>
                <TableRow>
                  <TableCell>Name</TableCell>
                  <TableCell>Kind</TableCell>
                  <TableCell>Required</TableCell>
                  <TableCell>Default</TableCell>
                  <TableCell>Description</TableCell>
                </TableRow>
              </TableHead>
              <TableBody>
                {preview.params.map((p) => (
                  <TableRow key={p.name}>
                    <TableCell sx={{ fontFamily: "monospace" }}>
                      {p.name}
                      {p.secret && (
                        <Chip
                          label="secret"
                          size="small"
                          sx={{ ml: 1 }}
                          variant="outlined"
                        />
                      )}
                    </TableCell>
                    <TableCell>{p.kind}</TableCell>
                    <TableCell>{p.required ? "yes" : "no"}</TableCell>
                    <TableCell
                      sx={{ fontFamily: "monospace", color: "text.secondary" }}
                    >
                      {p.default_value ?? "—"}
                    </TableCell>
                    <TableCell sx={{ color: "text.secondary" }}>
                      {p.description ?? "—"}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </TableContainer>
        )}
      </Section>

      <Section title="Actions" count={preview.actions.length}>
        {preview.actions.length > 0 && (
          <TableContainer component={Paper} variant="outlined">
            <Table size="small">
              <TableHead>
                <TableRow>
                  <TableCell>Name</TableCell>
                  <TableCell>Kind</TableCell>
                  <TableCell>Description</TableCell>
                </TableRow>
              </TableHead>
              <TableBody>
                {preview.actions.map((a) => (
                  <TableRow key={`${a.kind}/${a.name}`}>
                    <TableCell sx={{ fontFamily: "monospace" }}>{a.name}</TableCell>
                    <TableCell>
                      <Chip label={a.kind} size="small" variant="outlined" />
                    </TableCell>
                    <TableCell sx={{ color: "text.secondary" }}>
                      {a.description ?? "—"}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </TableContainer>
        )}
      </Section>
    </Stack>
  );
}

function Section({
  title,
  count,
  children,
}: {
  title: string;
  count: number;
  children: React.ReactNode;
}) {
  return (
    <Box>
      <Typography
        variant="caption"
        color="text.secondary"
        sx={{ display: "block", mb: 0.5 }}
      >
        {title} ({count})
      </Typography>
      {count === 0 ? (
        <Typography variant="body2" color="text.disabled" sx={{ fontStyle: "italic" }}>
          None declared.
        </Typography>
      ) : (
        children
      )}
    </Box>
  );
}

function resourceSummary(r: TemplatePreviewResource): string {
  const parts: string[] = [];
  if (r.scale) parts.push(`scale ${r.scale.low}..${r.scale.high}`);
  const def = r.def as Record<string, unknown> | undefined;
  if (def) {
    if (typeof def.image === "string") parts.push(def.image);
    const container = def.container as Record<string, unknown> | undefined;
    if (container && typeof container.image === "string") {
      parts.push(container.image as string);
    }
    if (typeof def.hostname === "string") parts.push(def.hostname as string);
    if (typeof def.service === "string" && typeof def.port === "number") {
      parts.push(`${def.service as string}:${def.port as number}`);
    }
  }
  return parts.join(" · ");
}
