import { Box, CircularProgress, Toolbar } from "@mui/material";
import { useContext } from "react";
import { Navigate, Outlet } from "react-router-dom";
import { Navbar } from "./Navbar";
import { SessionContext } from "./SessionProvider";

export function ProtectedRoute() {
  const { session, probing } = useContext(SessionContext);
  if (probing) return <CircularProgress sx={{ m: 4 }} />;
  if (!session) return <Navigate to="/login" replace />;
  return (
    <>
      <Navbar />
      <Box component="main">
        <Toolbar variant="dense" />
        <Outlet />
      </Box>
    </>
  );
}
