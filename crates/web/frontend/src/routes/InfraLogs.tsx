import ArticleIcon from "@mui/icons-material/Article";
import PauseIcon from "@mui/icons-material/Pause";
import PlayArrowIcon from "@mui/icons-material/PlayArrow";
import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Alert,
  Box,
  CircularProgress,
  FormControl,
  IconButton,
  InputLabel,
  MenuItem,
  Select,
  Tooltip,
  Typography,
} from "@mui/material";
import { useCallback, useContext, useEffect, useRef, useState } from "react";
import { Link, useParams } from "react-router-dom";
import { SessionContext } from "../components/SessionProvider";
import type { LogEntry } from "../lib/types";

const MAX_ENTRIES = 2000;
const TAIL_OPTIONS = [50, 100, 200, 500, 0] as const;

const COMPONENT_LABELS: Record<string, string> = {
  proxy: "Proxy (Caddy)",
  resolver: "Resolver (CoreDNS)",
};

function LogLine({ entry }: { entry: LogEntry }) {
  const ts = new Date(entry.timestamp);
  const time =
    ts.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" }) +
    "." +
    String(ts.getMilliseconds()).padStart(3, "0");
  const isErr = entry.stream === "stderr";

  return (
    <Box
      component="div"
      sx={{
        display: "flex",
        gap: 1,
        fontFamily: "monospace",
        fontSize: "0.78rem",
        lineHeight: 1.4,
        px: 1,
        py: 0.1,
        bgcolor: isErr ? "rgba(255,100,0,0.06)" : undefined,
        "&:hover": { bgcolor: isErr ? "rgba(255,100,0,0.1)" : "action.hover" },
        whiteSpace: "pre-wrap",
        wordBreak: "break-all",
      }}
    >
      <Box component="span" sx={{ color: "text.disabled", flexShrink: 0, userSelect: "none" }}>
        {time}
      </Box>
      {isErr && (
        <Box component="span" sx={{ color: "warning.main", flexShrink: 0, userSelect: "none" }}>
          err
        </Box>
      )}
      <Box component="span" sx={{ color: isErr ? "warning.light" : "text.primary", flexGrow: 1 }}>
        {entry.message}
      </Box>
    </Box>
  );
}

export default function InfraLogs() {
  const { component = "proxy" } = useParams<{ component: string }>();
  const [tail, setTail] = useState(100);
  const [follow, setFollow] = useState(true);
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [streaming, setStreaming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [streamKey, setStreamKey] = useState(0);

  const { session } = useContext(SessionContext);
  const scrollRef = useRef<HTMLDivElement>(null);
  const atBottomRef = useRef(true);

  const restart = useCallback(() => setStreamKey((k) => k + 1), []);

  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    atBottomRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  }, []);

  useEffect(() => {
    if (atBottomRef.current && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [entries]);

  useEffect(() => {
    if (!session) return;
    setEntries([]);
    setError(null);
    setStreaming(true);
    atBottomRef.current = true;

    const abort = new AbortController();
    session.client
      .streamLogs(
        { infra: component, follow, tail },
        (entry) =>
          setEntries((prev) => {
            const next = [...prev, entry];
            return next.length > MAX_ENTRIES ? next.slice(next.length - MAX_ENTRIES) : next;
          }),
        abort.signal,
      )
      .then(() => {
        if (!abort.signal.aborted) setStreaming(false);
      })
      .catch((e: unknown) => {
        if (!abort.signal.aborted) {
          setError(e instanceof Error ? e.message : String(e));
          setStreaming(false);
        }
      });

    return () => abort.abort();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session, component, follow, tail, streamKey]);

  const label = COMPONENT_LABELS[component] ?? component;

  return (
    <Box sx={{ height: "100%", display: "flex", flexDirection: "column" }}>
      <Box
        sx={{
          px: 2,
          py: 1,
          display: "flex",
          alignItems: "center",
          gap: 1,
          borderBottom: "1px solid",
          borderColor: "divider",
          flexShrink: 0,
        }}
      >
        <ArticleIcon fontSize="small" sx={{ color: "text.secondary" }} />
        <Typography variant="body2" sx={{ color: "text.primary" }}>
          {label}
        </Typography>
        <Typography variant="body2" color="text.disabled">
          — infra logs
        </Typography>
        <Typography
          component={Link}
          to="/"
          variant="body2"
          sx={{ ml: 1, textDecoration: "none", color: "text.secondary", "&:hover": { color: "text.primary" } }}
        >
          ← home
        </Typography>

        <Box sx={{ flexGrow: 1 }} />

        <FormControl size="small" sx={{ minWidth: 90 }}>
          <InputLabel>Tail</InputLabel>
          <Select value={tail} label="Tail" onChange={(e) => setTail(Number(e.target.value))}>
            {TAIL_OPTIONS.map((n) => (
              <MenuItem key={n} value={n}>
                {n === 0 ? "None" : n}
              </MenuItem>
            ))}
          </Select>
        </FormControl>

        <Tooltip title={follow ? "Pause" : "Follow"}>
          <IconButton size="small" onClick={() => setFollow((f) => !f)} color={follow ? "primary" : "default"}>
            {follow ? <PauseIcon fontSize="small" /> : <PlayArrowIcon fontSize="small" />}
          </IconButton>
        </Tooltip>

        <Tooltip title="Restart stream">
          <IconButton size="small" onClick={restart} disabled={streaming}>
            <RefreshIcon fontSize="small" />
          </IconButton>
        </Tooltip>

        {streaming && <CircularProgress size={16} />}
      </Box>

      {error && (
        <Alert severity="error" sx={{ m: 1, flexShrink: 0 }}>
          {error}
        </Alert>
      )}

      <Box
        ref={scrollRef}
        onScroll={onScroll}
        sx={{ flexGrow: 1, overflow: "auto", bgcolor: "grey.950", py: 0.5 }}
      >
        {entries.length === 0 && !streaming && !error && (
          <Typography variant="caption" color="text.disabled" sx={{ display: "block", p: 2 }}>
            No log entries.
          </Typography>
        )}
        {entries.map((entry, i) => (
          <LogLine key={i} entry={entry} />
        ))}
      </Box>
    </Box>
  );
}
