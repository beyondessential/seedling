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
import type { PlanDiffEntry, PlanResponse } from "../lib/types";

interface Props {
  plan: PlanResponse;
}

export function PlanDiff({ plan }: Props) {
  const diff = plan.diff ?? [];
  const handlers = plan.on_change_would_fire ?? [];
  const errors = plan.errors ?? [];

  return (
    <Stack spacing={2}>
      {errors.length > 0 &&
        errors.map((e, i) => (
          <Alert key={i} severity="error" sx={{ fontFamily: "monospace" }}>
            {e}
          </Alert>
        ))}

      <Box>
        <Typography
          variant="caption"
          color="text.secondary"
          sx={{ display: "block", mb: 0.5 }}
        >
          Resource changes ({diff.length})
        </Typography>
        {diff.length === 0 ? (
          <Typography
            variant="body2"
            color="text.disabled"
            sx={{ fontStyle: "italic" }}
          >
            No resource changes.
          </Typography>
        ) : (
          <TableContainer component={Paper} variant="outlined">
            <Table size="small">
              <TableHead>
                <TableRow>
                  <TableCell>Change</TableCell>
                  <TableCell>Type</TableCell>
                  <TableCell>Name</TableCell>
                  <TableCell>Fields</TableCell>
                </TableRow>
              </TableHead>
              <TableBody>
                {diff.map((entry) => (
                  <TableRow
                    key={`${entry.change}/${entry.resource_type}/${entry.resource_name}`}
                  >
                    <TableCell>
                      <Chip
                        label={entry.change}
                        size="small"
                        color={changeColor(entry.change)}
                        variant="outlined"
                      />
                    </TableCell>
                    <TableCell>
                      <Chip
                        label={entry.resource_type.toLowerCase()}
                        size="small"
                        variant="outlined"
                      />
                    </TableCell>
                    <TableCell sx={{ fontFamily: "monospace" }}>
                      {entry.resource_name}
                    </TableCell>
                    <TableCell
                      sx={{
                        fontFamily: "monospace",
                        color: "text.secondary",
                        fontSize: "0.75rem",
                      }}
                    >
                      {entry.fields?.join(", ") ?? ""}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </TableContainer>
        )}
      </Box>

      {handlers.length > 0 && (
        <Box>
          <Typography
            variant="caption"
            color="text.secondary"
            sx={{ display: "block", mb: 0.5 }}
          >
            on_change handlers that would fire ({handlers.length})
          </Typography>
          <Stack direction="row" spacing={0.5} sx={{ flexWrap: "wrap", gap: 0.5 }}>
            {handlers.map((h) => (
              <Chip
                key={h}
                label={h}
                size="small"
                sx={{ fontFamily: "monospace" }}
              />
            ))}
          </Stack>
        </Box>
      )}
    </Stack>
  );
}

function changeColor(change: PlanDiffEntry["change"]): "success" | "warning" | "error" | "default" {
  switch (change) {
    case "added":
      return "success";
    case "modified":
      return "warning";
    case "removed":
      return "error";
    default:
      return "default";
  }
}
