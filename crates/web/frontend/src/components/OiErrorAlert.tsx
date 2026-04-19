import { Alert, Box } from "@mui/material";
import type { OiQueryError } from "../hooks/useOi";

export function OiErrorAlert({ error }: { error: OiQueryError }) {
  return (
    <Alert severity="error">
      <Box>
        [OI] {error.method}: {error.message}
      </Box>
      {error.stack && (
        <details style={{ marginTop: 4 }}>
          <summary style={{ cursor: "pointer", userSelect: "none" }}>Stack trace</summary>
          <pre style={{ margin: "4px 0 0", fontSize: "0.75rem", overflowX: "auto", whiteSpace: "pre-wrap" }}>
            {error.stack}
          </pre>
        </details>
      )}
    </Alert>
  );
}
