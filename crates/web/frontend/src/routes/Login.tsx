import {
  Alert,
  Box,
  Button,
  CircularProgress,
  Container,
  Paper,
  TextField,
  Typography,
} from "@mui/material";
import { useContext, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";
import { SessionContext } from "../components/SessionProvider";
import { useLogin } from "../hooks/useSession";

export default function Login() {
  const { session } = useContext(SessionContext);
  const navigate = useNavigate();
  const { state, login } = useLogin();
  const [password, setPassword] = useState("");

  useEffect(() => {
    if (session) navigate("/", { replace: true });
  }, [session, navigate]);

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    void login(password);
  };

  return (
    <Container maxWidth="xs" sx={{ mt: 8 }}>
      <Paper elevation={3} sx={{ p: 4 }}>
        <Typography variant="h5" component="h1" gutterBottom align="center">
          Seedling
        </Typography>
        <Box component="form" onSubmit={handleSubmit} sx={{ mt: 2 }}>
          <TextField
            fullWidth
            label="Password"
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            disabled={state.status === "connecting"}
            margin="normal"
            autoFocus
          />
          {state.status === "error" && (
            <Alert severity="error" sx={{ mt: 1 }}>
              {state.message}
            </Alert>
          )}
          <Button
            type="submit"
            fullWidth
            variant="contained"
            sx={{ mt: 2 }}
            disabled={state.status === "connecting" || !password}
          >
            {state.status === "connecting" ? (
              <CircularProgress size={24} color="inherit" />
            ) : (
              "Sign in"
            )}
          </Button>
        </Box>
      </Paper>
    </Container>
  );
}
