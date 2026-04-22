import CleaningServicesIcon from "@mui/icons-material/CleaningServices";
import DeleteOutlineIcon from "@mui/icons-material/DeleteOutlineOutlined";
import LinkOffIcon from "@mui/icons-material/LinkOff";
import RefreshIcon from "@mui/icons-material/Refresh";
import {
  Alert,
  Box,
  Button,
  Chip,
  CircularProgress,
  Dialog,
  DialogActions,
  DialogContent,
  DialogTitle,
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
import { useMemo, useState } from "react";
import { Link } from "react-router-dom";
import {
  ImageReferencesCell,
  primaryReference,
} from "../components/ImageReferences";
import { OiErrorAlert } from "../components/OiErrorAlert";
import { useGuard } from "../components/SafetyModeProvider";
import { useOiQuery } from "../hooks/useOi";
import { useOiAction } from "../hooks/useOiAction";
import type { ImagePin, ImageSummary } from "../lib/types";

interface ImagesResponse {
  images: ImageSummary[];
}

interface PinsResponse {
  pins: ImagePin[];
}

function humanBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MiB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
}

function formatTimestamp(ts: string): string {
  try {
    return new Date(ts).toLocaleString();
  } catch {
    return ts;
  }
}

// w[impl routes.images]
export default function Images() {
  const { data, loading, error, refetch } = useOiQuery<ImagesResponse>(
    "/images/list",
    {},
  );
  const { data: pinsData, loading: pinsLoading, error: pinsError, refetch: refetchPins } =
    useOiQuery<PinsResponse>("/images/pins/list", {});
  const { execute, loading: mutating, error: mutateError, clearError } =
    useOiAction();
  const dangerGuard = useGuard("dangerous");
  const writeGuard = useGuard("write");

  const [removing, setRemoving] = useState<ImageSummary | null>(null);
  const [clearingPin, setClearingPin] = useState<ImagePin | null>(null);
  const [bulkOpen, setBulkOpen] = useState(false);

  const images = data?.images ?? [];
  const pins = pinsData?.pins ?? [];
  // Bulk sweep respects pins — a warmed image is an intentional declaration
  // that the image should stay even while no container is running. Operators
  // that really want to remove a pinned image can either clear the pin first
  // or remove the image from its row individually.
  const prunable = useMemo(
    () => images.filter((i) => !i.in_use && i.pinned_by.length === 0),
    [images],
  );

  const refreshAll = () => {
    refetch();
    refetchPins();
  };

  const submitRemove = async () => {
    if (!removing) return;
    try {
      await execute("/images/remove", {
        reference: primaryReference(removing),
      });
      setRemoving(null);
      refreshAll();
    } catch {
      /* surfaced via mutateError */
    }
  };

  const submitClearPin = async () => {
    if (!clearingPin) return;
    try {
      await execute("/images/pins/clear", {
        app: clearingPin.app,
        reference: clearingPin.reference,
      });
      setClearingPin(null);
      refreshAll();
    } catch {
      /* surfaced via mutateError */
    }
  };

  // w[impl routes.images]
  // Bulk "clear unused": iterate prunable images, remove each by its primary
  // reference. Stop on the first error so the operator can investigate.
  const submitBulk = async () => {
    let removed = 0;
    for (const img of prunable) {
      try {
        await execute("/images/remove", { reference: primaryReference(img) });
        removed += 1;
      } catch {
        break;
      }
    }
    if (removed > 0) refreshAll();
    setBulkOpen(false);
  };

  return (
    <Box sx={{ p: 3, maxWidth: 1100, mx: "auto" }}>
      <Box sx={{ display: "flex", alignItems: "center", mb: 2, gap: 1 }}>
        <Typography variant="h5" sx={{ flexGrow: 1 }}>
          Container Images
        </Typography>
        <Tooltip title="Refresh">
          <span>
            <IconButton
              onClick={refreshAll}
              disabled={loading || pinsLoading}
              size="small"
            >
              <RefreshIcon />
            </IconButton>
          </span>
        </Tooltip>
        <Tooltip
          title={
            !dangerGuard.allowed
              ? dangerGuard.reason ?? ""
              : prunable.length === 0
                ? "Nothing to clean up"
                : `Remove ${prunable.length} unused image${prunable.length === 1 ? "" : "s"}`
          }
        >
          <span>
            <Button
              variant="outlined"
              size="small"
              startIcon={<CleaningServicesIcon />}
              color="warning"
              onClick={() => {
                clearError();
                setBulkOpen(true);
              }}
              disabled={
                !dangerGuard.allowed || mutating || prunable.length === 0
              }
            >
              Clear unused
            </Button>
          </span>
        </Tooltip>
      </Box>
      <Typography
        variant="body2"
        sx={{ color: "text.secondary", mb: 2 }}
      >
        Images stored locally by the container runtime. Seedling also removes
        images that have not been used for 30 days on its own.
      </Typography>

      {error && <OiErrorAlert error={error} />}
      {(loading || pinsLoading) && !data && (
        <Box sx={{ display: "flex", justifyContent: "center", mt: 4 }}>
          <CircularProgress />
        </Box>
      )}

      {data && images.length === 0 && (
        <Alert severity="info">No container images in local storage.</Alert>
      )}

      {images.length > 0 && (
        <TableContainer component={Paper} variant="outlined" sx={{ mb: 3 }}>
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell>References</TableCell>
                <TableCell>Size</TableCell>
                <TableCell>Last used</TableCell>
                <TableCell>State</TableCell>
                <TableCell width={80} align="right" />
              </TableRow>
            </TableHead>
            <TableBody>
              {images.map((img) => (
                <TableRow key={img.image_id} hover>
                  <TableCell>
                    <ImageReferencesCell image={img} />
                  </TableCell>
                  <TableCell>{humanBytes(img.size_bytes)}</TableCell>
                  <TableCell>
                    <Typography
                      variant="caption"
                      sx={{ fontFamily: "monospace" }}
                    >
                      {formatTimestamp(img.last_used_at)}
                    </Typography>
                  </TableCell>
                  <TableCell>
                    <Stack direction="row" spacing={0.5}>
                      {img.in_use && (
                        <Chip label="in use" size="small" color="success" />
                      )}
                      {img.pinned_by.length > 0 && (
                        <Tooltip
                          title={`Pinned by: ${img.pinned_by.join(", ")}`}
                        >
                          <Chip
                            label={`pinned (${img.pinned_by.length})`}
                            size="small"
                            color="primary"
                            variant="outlined"
                          />
                        </Tooltip>
                      )}
                      {!img.in_use && img.pinned_by.length === 0 && (
                        <Chip
                          label="unused"
                          size="small"
                          variant="outlined"
                        />
                      )}
                    </Stack>
                  </TableCell>
                  <TableCell align="right">
                    <Tooltip
                      title={
                        !dangerGuard.allowed
                          ? dangerGuard.reason ?? ""
                          : img.in_use
                            ? "Cannot remove: image is in use"
                            : "Remove"
                      }
                    >
                      <span>
                        <IconButton
                          size="small"
                          onClick={() => {
                            clearError();
                            setRemoving(img);
                          }}
                          disabled={!dangerGuard.allowed || img.in_use}
                        >
                          <DeleteOutlineIcon fontSize="small" />
                        </IconButton>
                      </span>
                    </Tooltip>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </TableContainer>
      )}

      <Typography variant="h6" sx={{ mb: 1 }}>
        Pins
      </Typography>
      <Typography
        variant="body2"
        sx={{ color: "text.secondary", mb: 2 }}
      >
        Images apps have requested be kept around via <code>rt.warm_images</code>.
        A pin is cleared automatically once a running container picks up the
        image.
      </Typography>
      {pinsError && <OiErrorAlert error={pinsError} />}
      {pinsData && pins.length === 0 && (
        <Alert severity="info" sx={{ mb: 2 }}>
          No image pins.
        </Alert>
      )}
      {pins.length > 0 && (
        <TableContainer component={Paper} variant="outlined">
          <Table size="small">
            <TableHead>
              <TableRow>
                <TableCell>App</TableCell>
                <TableCell>Reference</TableCell>
                <TableCell>Pinned at</TableCell>
                <TableCell width={80} align="right" />
              </TableRow>
            </TableHead>
            <TableBody>
              {pins.map((p) => (
                <TableRow key={`${p.app}::${p.reference}`} hover>
                  <TableCell>
                    <Link
                      to={`/apps/${p.app}`}
                      style={{ fontFamily: "monospace" }}
                    >
                      {p.app}
                    </Link>
                  </TableCell>
                  <TableCell sx={{ fontFamily: "monospace" }}>
                    {p.reference}
                  </TableCell>
                  <TableCell>
                    <Typography
                      variant="caption"
                      sx={{ fontFamily: "monospace" }}
                    >
                      {formatTimestamp(p.pinned_at)}
                    </Typography>
                  </TableCell>
                  <TableCell align="right">
                    <Tooltip title={writeGuard.reason ?? "Clear pin"}>
                      <span>
                        <IconButton
                          size="small"
                          onClick={() => {
                            clearError();
                            setClearingPin(p);
                          }}
                          disabled={!writeGuard.allowed}
                        >
                          <LinkOffIcon fontSize="small" />
                        </IconButton>
                      </span>
                    </Tooltip>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </TableContainer>
      )}

      {/* w[impl routes.images.confirm] */}
      <Dialog
        open={removing !== null}
        onClose={() => setRemoving(null)}
        fullWidth
        maxWidth="sm"
      >
        <DialogTitle>Remove image</DialogTitle>
        <DialogContent>
          {removing && (
            <Stack spacing={2} sx={{ mt: 1 }}>
              <Typography>
                Remove <code>{primaryReference(removing)}</code> from local
                storage? This will fail if a running container is using the
                image.
              </Typography>
              {mutateError && <OiErrorAlert error={mutateError} />}
            </Stack>
          )}
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setRemoving(null)} disabled={mutating}>
            Cancel
          </Button>
          <Tooltip title={dangerGuard.reason ?? ""}>
            <span>
              <Button
                onClick={submitRemove}
                variant="contained"
                color="error"
                disabled={mutating || !dangerGuard.allowed}
              >
                Remove
              </Button>
            </span>
          </Tooltip>
        </DialogActions>
      </Dialog>

      <Dialog
        open={clearingPin !== null}
        onClose={() => setClearingPin(null)}
        fullWidth
        maxWidth="sm"
      >
        <DialogTitle>Clear pin</DialogTitle>
        <DialogContent>
          {clearingPin && (
            <Stack spacing={2} sx={{ mt: 1 }}>
              <Typography>
                Clear pin on <code>{clearingPin.reference}</code> for app{" "}
                <strong>{clearingPin.app}</strong>? The image stays in local
                storage but is no longer protected from autonomous GC.
              </Typography>
              {mutateError && <OiErrorAlert error={mutateError} />}
            </Stack>
          )}
        </DialogContent>
        <DialogActions>
          <Button
            onClick={() => setClearingPin(null)}
            disabled={mutating}
          >
            Cancel
          </Button>
          <Tooltip title={writeGuard.reason ?? ""}>
            <span>
              <Button
                onClick={submitClearPin}
                variant="contained"
                disabled={mutating || !writeGuard.allowed}
              >
                Clear pin
              </Button>
            </span>
          </Tooltip>
        </DialogActions>
      </Dialog>

      <Dialog
        open={bulkOpen}
        onClose={() => setBulkOpen(false)}
        fullWidth
        maxWidth="sm"
      >
        <DialogTitle>Clear unused images</DialogTitle>
        <DialogContent>
          <Stack spacing={2} sx={{ mt: 1 }}>
            <Typography>
              Remove every image that is not currently backing a running
              container and is not pinned. Pinned images are left alone —
              clear their pin first if you want them gone.
            </Typography>
            <Typography variant="body2" sx={{ color: "text.secondary" }}>
              {prunable.length} image{prunable.length === 1 ? "" : "s"} will be
              removed.
            </Typography>
            {mutateError && <OiErrorAlert error={mutateError} />}
          </Stack>
        </DialogContent>
        <DialogActions>
          <Button onClick={() => setBulkOpen(false)} disabled={mutating}>
            Cancel
          </Button>
          <Tooltip title={dangerGuard.reason ?? ""}>
            <span>
              <Button
                onClick={submitBulk}
                variant="contained"
                color="warning"
                disabled={mutating || !dangerGuard.allowed}
              >
                Remove {prunable.length}
              </Button>
            </span>
          </Tooltip>
        </DialogActions>
      </Dialog>
    </Box>
  );
}
