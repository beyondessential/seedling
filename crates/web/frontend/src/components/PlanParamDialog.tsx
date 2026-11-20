import { Button, CircularProgress, Dialog, DialogActions, DialogContent, DialogTitle, Box } from "@mui/material";
import { useEffect, useState } from "react";
import { useOiAction } from "../hooks/useOiAction";
import type { ParamChanges, PlanResponse } from "../lib/types";
import { OiErrorAlert } from "./OiErrorAlert";
import { PlanDiff } from "./PlanDiff";

/** What setting some parameters and unsetting others, as one change, would
 *  change, planned without applying anything. */
// spec: UI#operator-capabilities
export function PlanParamDialog({
  appName,
  changes,
  onClose,
}: {
  appName: string;
  changes: ParamChanges;
  onClose: () => void;
}) {
  const { execute, error } = useOiAction();
  const [plan, setPlan] = useState<PlanResponse | null>(null);

  const names = Object.keys(changes).sort().join(", ");
  const proposed = JSON.stringify(changes);

  useEffect(() => {
    let cancelled = false;
    void execute("/apps/plan", {
      app: appName,
      proposed_params: JSON.parse(proposed) as ParamChanges,
    }).then((r) => {
      if (!cancelled && r) setPlan(r as PlanResponse);
    });
    return () => {
      cancelled = true;
    };
  }, [execute, appName, proposed]);

  return (
    <Dialog open onClose={onClose} maxWidth="md" fullWidth>
      <DialogTitle>
        Plan change ·{" "}
        <Box component="span" sx={{ fontFamily: "monospace" }}>
          {names}
        </Box>
      </DialogTitle>
      <DialogContent dividers>
        {error && <OiErrorAlert error={error} />}
        {!plan && !error && <CircularProgress size={20} />}
        {plan && <PlanDiff plan={plan} />}
      </DialogContent>
      <DialogActions>
        <Button onClick={onClose}>Close</Button>
      </DialogActions>
    </Dialog>
  );
}
