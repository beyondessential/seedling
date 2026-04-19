import { AppBar, Box, Chip, Toolbar, Typography } from "@mui/material";
import { useOiQuery } from "../hooks/useOi";
import { useSessionContext } from "./SessionProvider";

interface StatusSummary {
  hostname: string;
  version: string;
}

export function Navbar() {
  const { data } = useOiQuery<StatusSummary>("/server/status", {});
  const { reconnecting } = useSessionContext();

  return (
    <AppBar position="fixed">
      <Toolbar variant="dense">
        <Typography sx={{ mr: 1, fontSize: "1.2rem", lineHeight: 1 }}>
          🌱
        </Typography>
        <Typography variant="h6" sx={{ fontWeight: 700, letterSpacing: "-0.5px" }}>
          Seedling
        </Typography>
        <Box sx={{ flexGrow: 1 }} />
        {reconnecting && (
          <Chip
            label="Reconnecting…"
            size="small"
            color="warning"
            sx={{ mr: 1, fontFamily: "monospace" }}
          />
        )}
        {data?.hostname && (
          <Typography variant="body2" sx={{ opacity: 0.85, fontFamily: "monospace" }}>
            {data.hostname}
          </Typography>
        )}
      </Toolbar>
    </AppBar>
  );
}
