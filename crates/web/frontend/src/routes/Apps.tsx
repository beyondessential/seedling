import { Alert, CircularProgress, Typography } from "@mui/material";
import { useOiQuery } from "../hooks/useOi";

export default function Apps() {
  const { data, loading, error } = useOiQuery<unknown>("/apps/list", {});

  if (loading) return <CircularProgress sx={{ m: 4 }} />;
  if (error) return <Alert severity="error" sx={{ m: 2 }}>{error}</Alert>;

  return (
    <Typography variant="body1" sx={{ p: 2 }}>
      {data ? JSON.stringify(data, null, 2) : "No data."}
    </Typography>
  );
}
