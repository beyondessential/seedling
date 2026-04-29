import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Box,
  Chip,
  CircularProgress,
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
import { useEffect } from "react";
import { Link as RouterLink } from "react-router-dom";
import { useGuard } from "./SafetyModeProvider";
import { useOiQuery } from "../hooks/useOi";
import { useOiAction } from "../hooks/useOiAction";
import { OiErrorAlert } from "./OiErrorAlert";
import type {
  TlsHostnameLastIssuance,
  TlsHostnameStatus,
  TlsHostnameView,
  TlsHostnamesResponse,
} from "../lib/types";

interface TlsHostnamesTableProps {
  /** Filter to a single app's TLS-terminating domains. */
  app?: string;
  /** Hide the "Apps" column (useful when filtered to a single app). */
  hideAppsColumn?: boolean;
  /** Override the section title shown above the table. */
  title?: string;
  /** Hide the section title entirely (for callers that render their own). */
  hideTitle?: boolean;
}

const STATUS_LABEL: Record<TlsHostnameStatus, string> = {
  active: "active",
  expired: "expired",
  error: "error",
  pending: "pending",
  blocked: "blocked",
  no_cert: "no cert",
  default: "default (proxy)",
};

const STATUS_COLOR: Record<
  TlsHostnameStatus,
  "default" | "primary" | "success" | "warning" | "error"
> = {
  active: "success",
  expired: "error",
  error: "error",
  pending: "warning",
  blocked: "warning",
  no_cert: "default",
  default: "default",
};

function formatTime(unix: number | null): string {
  if (unix == null) return "—";
  return new Date(unix * 1000).toLocaleString();
}

function relative(unix: number, now: number): string {
  const dt = unix - now;
  const abs = Math.abs(dt);
  const fmt = (n: number, unit: string) =>
    `${dt < 0 ? "" : "in "}${n}${unit}${dt < 0 ? " ago" : ""}`;
  if (abs < 60) return fmt(abs, "s");
  if (abs < 3600) return fmt(Math.round(abs / 60), "m");
  if (abs < 86400) return fmt(Math.round(abs / 3600), "h");
  return fmt(Math.round(abs / 86400), "d");
}

function policySummary(view: TlsHostnameView): string {
  const policy = view.policy;
  switch (policy.strategy) {
    case "default": {
      const caddy = view.active_cert?.caddy_issuer;
      if (caddy) return `default — ${caddyIssuerLabel(caddy)}`;
      return "default — Caddy automatic TLS";
    }
    case "acme_dns":
      return policy.is_wildcard_match
        ? `ACME-DNS via ${policy.dns_provider} (${policy.pattern})`
        : `ACME-DNS via ${policy.dns_provider}`;
  }
}

function caddyIssuerLabel(issuer: string): string {
  if (issuer === "local") return "Caddy internal CA";
  if (issuer.startsWith("acme-")) return `Caddy ACME (${issuer})`;
  return `Caddy (${issuer})`;
}

function lastIssuanceLabel(
  last: TlsHostnameLastIssuance | null,
  now: number,
): { primary: string; secondary: string | null } {
  if (!last) return { primary: "—", secondary: null };
  const when = last.at
    ? `${formatTime(last.at)} (${relative(last.at, now)})`
    : null;
  if (last.kind === "manual") {
    return { primary: "manual upload", secondary: when };
  }
  if (last.kind === "csr") {
    return { primary: "CSR-derived", secondary: when };
  }
  if (last.kind === "caddy") {
    return { primary: caddyIssuerLabel(last.provider), secondary: when };
  }
  const provider = last.provider ?? "unknown provider";
  return { primary: `ACME-DNS via ${provider}`, secondary: when };
}

function nextIssuanceLabel(view: TlsHostnameView, now: number): React.ReactNode {
  if (view.next_issuance_source === "immediate") return "queued";
  if (view.next_issuance_at == null) {
    // No runtime issuance schedule applies — Caddy is the one driving
    // renewal (default strategy: internal CA or ACME via the proxy).
    if (view.policy.strategy === "default") return "controlled by Caddy";
    return "—";
  }
  if (view.next_issuance_source === "debounce") {
    // Last attempt failed; the runtime won't retry until this point.
    return `retry after ${relative(view.next_issuance_at, now)} (last attempt failed)`;
  }
  const when = relative(view.next_issuance_at, now);
  if (view.next_issuance_source === "ari") {
    return (
      <>
        {when}{" "}
        (
        <abbr title="ACME Renewal Information (RFC 9773): the issuing CA told us when to renew this cert.">
          ARI
        </abbr>
        )
      </>
    );
  }
  if (view.next_issuance_source === "fallback") {
    return `${when} (fallback)`;
  }
  return when;
}

export function TlsHostnamesTable({
  app,
  hideAppsColumn,
  title,
  hideTitle,
}: TlsHostnamesTableProps) {
  const params = app ? { app } : {};
  const { data, loading, error, refetch } =
    useOiQuery<TlsHostnamesResponse>("/tls/hostnames/list", params);
  const { execute, error: actionError, clearError } = useOiAction();
  const writeGuard = useGuard("write");

  // Auto-refresh so newly-arriving attempts/blocks/cert state surface
  // without operator interaction. 5s feels live, is cheap.
  useEffect(() => {
    const t = setInterval(refetch, 5000);
    return () => clearInterval(t);
  }, [refetch]);

  const onRetry = async (hostname: string) => {
    clearError();
    try {
      await execute("/tls/certificates/retry", { hostname });
      refetch();
    } catch {
      // surfaced via actionError
    }
  };

  const rows = data?.hostnames ?? [];
  const now = Math.floor(Date.now() / 1000);

  return (
    <Box>
      {!hideTitle && (
        <Typography variant="subtitle1" sx={{ fontWeight: 600, mb: 1 }}>
          {title ?? "Domains"}
        </Typography>
      )}
      {error && <OiErrorAlert error={error} />}
      {actionError && <OiErrorAlert error={actionError} />}
      {loading && rows.length === 0 && <CircularProgress size={20} />}
      {!loading && rows.length === 0 ? (
        <Typography variant="body2" sx={{ color: "text.secondary" }}>
          No TLS-terminating ingress domains declared.
        </Typography>
      ) : (
        <TableContainer component={Paper} variant="outlined">
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell>Domain</TableCell>
                {!hideAppsColumn && <TableCell>Apps</TableCell>}
                <TableCell>Status</TableCell>
                <TableCell>Last issued</TableCell>
                <TableCell>Next issuance</TableCell>
                <TableCell align="right" />
              </TableRow>
            </TableHead>
            <TableBody>
              {rows.map((row) => {
                const last = lastIssuanceLabel(row.last_issuance, now);
                const isAcmeDns = row.policy.strategy === "acme_dns";
                const statusTooltip =
                  row.status === "error" && row.last_error
                    ? row.last_error
                    : row.status === "blocked" && row.retry_block?.reason
                      ? `Paused: ${row.retry_block.reason}`
                      : row.status === "blocked"
                        ? "Issuance is paused for this domain"
                        : row.status === "default"
                          ? "No policy bound — the proxy handles ACME-HTTP-01 automatically"
                          : row.status === "pending" && row.force_retry_at
                            ? "Retry queued"
                            : "";
                return (
                  <TableRow key={row.hostname} hover>
                    <TableCell sx={{ fontFamily: "monospace" }}>
                      {row.hostname}
                    </TableCell>
                    {!hideAppsColumn && (
                      <TableCell>
                        <Stack
                          direction="row"
                          spacing={0.5}
                          sx={{ flexWrap: "wrap", rowGap: 0.5 }}
                        >
                          {row.apps.map((a) => (
                            <Chip
                              key={a}
                              label={a}
                              size="small"
                              variant="outlined"
                              component={RouterLink}
                              to={`/apps/${a}`}
                              clickable
                            />
                          ))}
                        </Stack>
                      </TableCell>
                    )}
                    <TableCell>
                      <Stack
                        direction="row"
                        spacing={0.5}
                        sx={{ alignItems: "center", flexWrap: "wrap" }}
                      >
                        <Tooltip title={statusTooltip}>
                          <Chip
                            label={STATUS_LABEL[row.status]}
                            size="small"
                            color={STATUS_COLOR[row.status]}
                            variant={
                              row.status === "active" ? "filled" : "outlined"
                            }
                          />
                        </Tooltip>
                        {row.active_cert?.self_signed && (
                          <Chip
                            label="self-signed"
                            size="small"
                            color="warning"
                            variant="outlined"
                          />
                        )}
                      </Stack>
                      <Typography
                        variant="caption"
                        sx={{ color: "text.secondary", display: "block" }}
                      >
                        {policySummary(row)}
                      </Typography>
                    </TableCell>
                    <TableCell>
                      <Typography variant="body2">{last.primary}</Typography>
                      {last.secondary && (
                        <Typography
                          variant="caption"
                          sx={{ color: "text.secondary" }}
                        >
                          {last.secondary}
                        </Typography>
                      )}
                    </TableCell>
                    <TableCell>
                      <Typography variant="body2">
                        {nextIssuanceLabel(row, now)}
                      </Typography>
                      {row.active_cert?.not_after && (
                        <Typography
                          variant="caption"
                          sx={{ color: "text.secondary" }}
                        >
                          {row.active_cert.not_after < now ? "expired" : "expires"}{" "}
                          {relative(row.active_cert.not_after, now)}
                        </Typography>
                      )}
                    </TableCell>
                    <TableCell align="right">
                      {isAcmeDns && (
                        <Tooltip title={writeGuard.title("Renew now")}>
                          <span>
                            <IconButton
                              size="small"
                              disabled={!writeGuard.allowed}
                              onClick={() => onRetry(row.hostname)}
                            >
                              <RefreshIcon fontSize="small" />
                            </IconButton>
                          </span>
                        </Tooltip>
                      )}
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        </TableContainer>
      )}
    </Box>
  );
}
