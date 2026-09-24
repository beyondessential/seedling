import {
  Alert,
  Box,
  Button,
  Paper,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableRow,
  Typography,
} from "@mui/material";
import { useState } from "react";
import type {
  DefinitionActor,
  DefinitionProvenance,
  FaultRecord,
} from "../lib/types";
import { OutlinedActionButton } from "./ActionButton";
import { UpdateDefinitionDialog } from "./UpdateDefinitionDialog";

const mono = { fontFamily: "monospace" } as const;

function actorLabel(actor: DefinitionActor | null): string {
  if (!actor) return "unknown";
  const who = actor.display ?? actor.id ?? "unknown";
  return actor.kind ? `${who} (${actor.kind})` : who;
}

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <TableRow>
      <TableCell sx={{ color: "text.secondary", width: 140 }}>{label}</TableCell>
      <TableCell>{children}</TableCell>
    </TableRow>
  );
}

/** Where the app's current definition came from, and the way to replace it
 *  from a registry. */
// w[impl routes.apps.definition]
export function DefinitionSection({
  appName,
  definition,
  faults,
  onUpdated,
}: {
  appName: string;
  definition: DefinitionProvenance;
  faults: FaultRecord[];
  onUpdated: () => void;
}) {
  const [dialogOpen, setDialogOpen] = useState(false);
  const moved = faults.find((f) => f.kind === "definition_source_moved");
  const recordedReference =
    definition.kind === "fetched" ? definition.reference : null;

  return (
    <Box>
      <Box
        sx={{
          mb: 1,
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: 1,
        }}
      >
        <Typography variant="h6">Definition</Typography>
        <OutlinedActionButton
          safety="write"
          size="small"
          onClick={() => setDialogOpen(true)}
        >
          Update from registry
        </OutlinedActionButton>
      </Box>
      <TableContainer component={Paper} variant="outlined">
        <Table size="small">
          <TableBody>
            {definition.kind === "fetched" ? (
              <>
                <Row label="Source">
                  <Box component="span" sx={mono}>
                    {definition.reference}
                  </Box>
                </Row>
                <Row label="Digest">
                  <Box component="span" sx={mono}>
                    {definition.digest}
                  </Box>
                </Row>
              </>
            ) : (
              <>
                <Row label="Source">
                  Pushed by <strong>{actorLabel(definition.pushed_by)}</strong>
                </Row>
                <Row label="Content hash">
                  <Box component="span" sx={mono}>
                    {definition.content_hash}
                  </Box>
                </Row>
                {definition.reported_origin && (
                  <Row label="Origin">
                    <Box component="span" sx={mono}>
                      {definition.reported_origin.url}
                    </Box>{" "}
                    <Box component="span" sx={{ ...mono, color: "text.secondary" }}>
                      @ {definition.reported_origin.revision}
                    </Box>{" "}
                    <Typography variant="caption" sx={{ color: "text.secondary" }}>
                      (reported by client)
                    </Typography>
                  </Row>
                )}
              </>
            )}
            {definition.seedling_versions && (
              <Row label="Seedling">
                <Box component="span" sx={mono}>
                  {definition.seedling_versions}
                </Box>
              </Row>
            )}
          </TableBody>
        </Table>
      </TableContainer>
      {/* w[impl routes.apps.definition.fetch] */}
      {moved && (
        <Alert
          severity="warning"
          sx={{ mt: 1 }}
          action={
            <Button color="inherit" size="small" onClick={() => setDialogOpen(true)}>
              Review
            </Button>
          }
        >
          {moved.description}
        </Alert>
      )}
      {dialogOpen && (
        <UpdateDefinitionDialog
          appName={appName}
          recordedReference={recordedReference}
          preselect={moved ? recordedReference : null}
          onClose={() => setDialogOpen(false)}
          onApplied={() => {
            setDialogOpen(false);
            onUpdated();
          }}
        />
      )}
    </Box>
  );
}
