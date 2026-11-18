import {
  Box,
  Button,
  CircularProgress,
  Paper,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  Typography,
} from "@mui/material";
import { useCallback, useEffect, useState } from "react";
import { useEventRefresh } from "../hooks/useEventRefresh";
import { useOiQuery } from "../hooks/useOi";
import { useOiAction } from "../hooks/useOiAction";
import type { DefinitionProvenance, GenerationEntry, SeedlingEvent } from "../lib/types";
import { OiErrorAlert } from "./OiErrorAlert";

const PAGE = 50;
const mono = { fontFamily: "monospace" } as const;

const GENERATION_EVENTS: Set<string> = new Set([
  "AppRegistered",
  "AppUpdated",
  "ParamsChanged",
]);

const KIND_LABEL: Record<GenerationEntry["kind"], string> = {
  register: "registered",
  script_update: "script updated",
  param_change: "params changed",
};

function provenanceLabel(def: DefinitionProvenance): string {
  return def.kind === "fetched"
    ? def.reference
    : `pushed ${def.content_hash.slice(0, 12)}`;
}

function shown(value: string | null | undefined): string {
  return value === null || value === undefined ? "(unset)" : value;
}

function Detail({ entry }: { entry: GenerationEntry }) {
  return (
    <>
      {entry.definition && (
        <Box component="div" sx={mono}>
          {provenanceLabel(entry.definition)}
        </Box>
      )}
      {entry.params?.map((p) => (
        <Box component="div" sx={mono} key={p.name}>
          {p.name}
          {p.redacted ? (
            <Typography component="span" variant="caption" sx={{ color: "text.secondary", ml: 1 }}>
              values withheld
            </Typography>
          ) : (
            <>
              {": "}
              {shown(p.previous_value)} → {shown(p.new_value)}
            </>
          )}
        </Box>
      ))}
    </>
  );
}

/** The changes made to an app, newest first. */
// spec: UI#operator-capabilities
export function GenerationHistory({ appName }: { appName: string }) {
  const first = useOiQuery<GenerationEntry[]>("/apps/generations", { app: appName, limit: PAGE });
  const { execute, loading: loadingMore, error: moreError } = useOiAction();
  const [older, setOlder] = useState<GenerationEntry[]>([]);
  const [exhausted, setExhausted] = useState(false);

  const matches = useCallback(
    (ev: SeedlingEvent) => GENERATION_EVENTS.has(ev.type) && (!ev.app || ev.app === appName),
    [appName],
  );
  useEventRefresh(first.refetch, matches);

  useEffect(() => {
    setOlder([]);
    setExhausted(false);
  }, [first.data]);

  const entries = [...(first.data ?? []), ...older];
  const lastPageFull =
    older.length > 0 ? !exhausted : (first.data?.length ?? 0) >= PAGE && !exhausted;

  const loadMore = async () => {
    const last = entries[entries.length - 1];
    const page = (await execute("/apps/generations", {
      app: appName,
      limit: PAGE,
      before: last.generation,
    })) as GenerationEntry[] | null;
    if (page === null) return;
    setOlder((o) => [...o, ...page]);
    if (page.length < PAGE) setExhausted(true);
  };

  if (first.error) return <OiErrorAlert error={first.error} />;
  if (first.loading && !first.data) return <CircularProgress size={20} />;
  if (entries.length === 0) {
    return (
      <Typography sx={{ color: "text.secondary" }}>No generations.</Typography>
    );
  }

  return (
    <Box>
      <TableContainer component={Paper} variant="outlined">
        <Table size="small">
          <TableHead>
            <TableRow>
              <TableCell width={80}>Generation</TableCell>
              <TableCell>When</TableCell>
              <TableCell>Change</TableCell>
            </TableRow>
          </TableHead>
          <TableBody>
            {entries.map((e) => (
              <TableRow key={e.generation}>
                <TableCell sx={mono}>{e.generation}</TableCell>
                <TableCell sx={{ color: "text.secondary" }}>
                  {new Date(e.timestamp).toLocaleString()}
                </TableCell>
                <TableCell>
                  <Box>{KIND_LABEL[e.kind]}</Box>
                  <Detail entry={e} />
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </TableContainer>
      {moreError && <OiErrorAlert error={moreError} />}
      {lastPageFull && (
        <Button size="small" sx={{ mt: 1 }} onClick={() => void loadMore()} disabled={loadingMore}>
          Show older
        </Button>
      )}
    </Box>
  );
}
