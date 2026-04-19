import { CircularProgress } from "@mui/material";
import { useContext } from "react";
import { Navigate, Outlet } from "react-router-dom";
import { SessionContext } from "./SessionProvider";

export function ProtectedRoute() {
  const { session, probing } = useContext(SessionContext);
  if (probing) return <CircularProgress sx={{ m: 4 }} />;
  if (!session) return <Navigate to="/login" replace />;
  return <Outlet />;
}
