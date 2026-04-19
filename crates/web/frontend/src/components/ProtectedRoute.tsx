import { useContext } from "react";
import { Navigate, Outlet } from "react-router-dom";
import { SessionContext } from "./SessionProvider";

export function ProtectedRoute() {
  const { session } = useContext(SessionContext);
  if (!session) return <Navigate to="/login" replace />;
  return <Outlet />;
}
