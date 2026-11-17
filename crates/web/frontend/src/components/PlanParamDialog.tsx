import { Button, CircularProgress, Dialog, DialogActions, DialogContent, DialogTitle, Box } from "@mui/material";
import { useEffect, useState } from "react";
import { useOiAction } from "../hooks/useOiAction";
import type { PlanResponse } from "../lib/types";
import { OiErrorAlert } from "./OiErrorAlert";
import { PlanDiff } from "./PlanDiff";

/** What setting, or with a null value unsetting, one parameter would change,
 *  planned without applying anything. */
// spec: UI#operator-capabilities
export function PlanParamDialog({
  appName,
  name,
  value,
  onClose,
}: {
  appName: string;
  name: string;
  value: string | null;
  onClose: () => void;
}) {
  const { execute, error } = useOiAction();
  const [plan, setPlan] = useState<PlanResponse | null>(null);

  useEffect(() => {
    let cancelled = false;
    void execute("/apps/plan", {
      app: appName,
      proposed_params: [{ name, value }],
    }).then((r) => {
      if (!cancelled && r) setPlan(r as PlanResponse);
    });
    return () => {
      cancelled = true;
    };
  }, [execute, appName, name, value]);

  return (
    <Dialog open onClose={onClose} maxWidth="md" fullWidth>
      <DialogTitle>
        Plan change ·{" "}
        <Box component="span" sx={{ fontFamily: "monospace" }}>
          {name}
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
