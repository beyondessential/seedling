import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Alert,
  Box,
  Chip,
  CircularProgress,
  Divider,
  IconButton,
  Paper,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  Tooltip,
  Typography,
} from "@mui/material";
import { Link, useParams } from "react-router-dom";
import { useOiQuery } from "../hooks/useOi";
import { statusColor, statusLabel } from "../lib/status";
import type {
  AppAction,
  AppDetail,
  AppParam,
  AppResource,
  FaultRecord,
} from "../lib/types";

function lifecycleColor(
  state: string,
): "success" | "warning" | "error" | "default" {
  if (state === "active") return "success";
  if (state === "failed") return "error";
  if (state === "excluded") return "warning";
  return "default";
}

function FaultList({ faults }: { faults: FaultRecord[] }) {
  if (faults.length === 0) return null;
  return (
    <Stack spacing={1}>
      {faults.map((f) => (
        <Alert key={f.id} severity="error" sx={{ fontFamily: "monospace" }}>
          <strong>{f.kind}</strong>
          {f.resource_name && ` · ${f.resource_type}/${f.resource_name}`}
          {f.instance_id && ` (${f.instance_id})`}
          {" — "}
          {f.description}
        </Alert>
      ))}
    </Stack>
  );
}

function ResourcesSection({ resources }: { resources: AppResource[] }) {
  if (resources.length === 0) return <Typography color="text.secondary">No resources.</Typography>;
  return (
    <Stack spacing={2}>
      {resources.map((r) => (
        <Box key={r.name}>
          <Box sx={{ display: "flex", alignItems: "center", gap: 1, mb: 0.5 }}>
            <Typography variant="subtitle2">{r.name}</Typography>
            <Typography variant="caption" color="text.secondary">
              {r.type}
            </Typography>
            {r.scale && (
              <Typography variant="caption" color="text.secondary">
                · scale {r.scale.current} [{r.scale.low}–{r.scale.high}]
              </Typography>
            )}
          </Box>
          <FaultList faults={r.faults} />
          <TableContainer component={Paper} variant="outlined">
            <Table size="small">
              <TableHead>
                <TableRow>
                  <TableCell>Instance</TableCell>
                  <TableCell>State</TableCell>
                </TableRow>
              </TableHead>
              <TableBody>
                {r.instances.length === 0 ? (
                  <TableRow>
                    <TableCell colSpan={2} sx={{ color: "text.secondary" }}>
                      No instances.
                    </TableCell>
                  </TableRow>
                ) : (
                  r.instances.map((inst) => (
                    <TableRow key={inst.id}>
                      <TableCell sx={{ fontFamily: "monospace", fontSize: "0.8rem" }}>
                        {inst.display_name}
                      </TableCell>
                      <TableCell>
                        <Chip
                          label={inst.lifecycle.replace(/_/g, " ")}
                          color={lifecycleColor(inst.lifecycle)}
                          size="small"
                        />
                      </TableCell>
                    </TableRow>
                  ))
                )}
              </TableBody>
            </Table>
          </TableContainer>
        </Box>
      ))}
    </Stack>
  );
}

function ParamsSection({ params }: { params: AppParam[] }) {
  if (params.length === 0) return <Typography color="text.secondary">No params.</Typography>;
  return (
    <TableContainer component={Paper} variant="outlined">
      <Table size="small">
        <TableHead>
          <TableRow>
            <TableCell>Name</TableCell>
            <TableCell>Value</TableCell>
          </TableRow>
        </TableHead>
        <TableBody>
          {params.map((p) => (
            <TableRow key={p.name}>
              <TableCell sx={{ fontFamily: "monospace" }}>{p.name}</TableCell>
              <TableCell sx={{ fontFamily: "monospace", color: p.value == null ? "text.disabled" : undefined }}>
                {p.value ?? "—"}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </TableContainer>
  );
}

function ActionsSection({ actions }: { actions: AppAction[] }) {
  if (actions.length === 0) return <Typography color="text.secondary">No actions.</Typography>;
  return (
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
          {actions.map((a) => (
            <TableRow key={a.name}>
              <TableCell sx={{ fontFamily: "monospace" }}>{a.name}</TableCell>
              <TableCell>
                <Chip label={a.kind} size="small" variant="outlined" />
              </TableCell>
              <TableCell sx={{ color: "text.secondary" }}>{a.description}</TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </TableContainer>
  );
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <Box>
      <Typography variant="h6" sx={{ mb: 1 }}>
        {title}
      </Typography>
      {children}
    </Box>
  );
}

export default function AppDetail() {
  const { name } = useParams<{ name: string }>();
  const { data, loading, error, refetch } = useOiQuery<AppDetail>(
    "/apps/show",
    { app: name },
  );

  return (
    <Box sx={{ p: 3, maxWidth: 900, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography
          component={Link}
          to="/"
          variant="body2"
          sx={{ color: "text.secondary", textDecoration: "none", "&:hover": { textDecoration: "underline" } }}
        >
          Apps
        </Typography>
        <Typography variant="body2" color="text.disabled">/</Typography>
        <Typography variant="body2">{name}</Typography>
        <Box sx={{ flexGrow: 1 }} />
        <Tooltip title="Refresh">
          <span>
            <IconButton onClick={refetch} disabled={loading} size="small">
              <RefreshIcon />
            </IconButton>
          </span>
        </Tooltip>
      </Box>

      {error && (
        <Alert severity="error" sx={{ mb: 2 }}>
          {error}
        </Alert>
      )}

      {loading && !data && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}

      {data && (
        <Stack spacing={3}>
          <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
            <Typography variant="h5">{name}</Typography>
            <Chip
              label={statusLabel(data.status, data.current_operation?.action_name)}
              color={statusColor(data.status)}
              size="small"
            />
            <Typography variant="caption" color="text.secondary">
              gen {data.generation}
            </Typography>
          </Box>

          {data.current_operation && (
            <Alert severity="info">
              Operation in progress: <strong>{data.current_operation.action_name}</strong>
              {" "}(gen {data.current_operation.source_generation} → {data.current_operation.target_generation})
              {data.current_operation.barrier && (
                <> · barrier: {data.current_operation.barrier.required_state}
                  {" "}({Math.round(data.current_operation.barrier.elapsed_secs)}s
                  {" "}/ {data.current_operation.barrier.deadline_secs}s)</>
              )}
            </Alert>
          )}

          {data.faults.length > 0 && (
            <Section title="Faults">
              <FaultList faults={data.faults} />
            </Section>
          )}

          <Divider />

          <Section title="Resources">
            <ResourcesSection resources={data.resources} />
          </Section>

          <Divider />

          <Section title="Params">
            <ParamsSection params={data.params} />
          </Section>

          <Divider />

          <Section title="Actions">
            <ActionsSection actions={data.actions} />
          </Section>
        </Stack>
      )}
    </Box>
  );
}
