import { Alert, Box, Typography } from "@mui/material";
import { useRouteError } from "react-router-dom";

export default function ErrorPage() {
  const error = useRouteError();
  const message =
    error instanceof Error
      ? error.message
      : typeof error === "object" && error !== null && "statusText" in error
        ? String((error as { statusText: unknown }).statusText)
        : String(error);

  return (
    <Box sx={{ p: 4, maxWidth: 600, mx: "auto" }}>
      <Typography variant="h5" gutterBottom>
        Something went wrong
      </Typography>
      <Alert severity="error">{message}</Alert>
    </Box>
  );
}
