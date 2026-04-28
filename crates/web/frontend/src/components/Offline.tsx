import { Box, CircularProgress, Container, Paper, Typography } from "@mui/material";

export function Offline() {
  return (
    <Container maxWidth="sm" sx={{ mt: 8 }}>
      <Paper elevation={3} sx={{ p: 4 }}>
        <Typography variant="h5" component="h1" gutterBottom align="center">
          Can't reach Seedling
        </Typography>
        <Typography variant="body2" color="text.secondary" align="center" sx={{ mb: 3 }}>
          The daemon isn't responding. We'll keep trying — this page will reload automatically once it's back.
        </Typography>
        <Box sx={{ display: "flex", justifyContent: "center" }}>
          <CircularProgress size={28} />
        </Box>
      </Paper>
    </Container>
  );
}
