import { Box, CircularProgress, Toolbar } from "@mui/material";
import { useContext } from "react";
import { Navigate, Outlet } from "react-router-dom";
import { EventsSidebar } from "./EventsSidebar";
import { Navbar } from "./Navbar";
import { SessionContext } from "./SessionProvider";
import { ShellsSidebar } from "./ShellsSidebar";

export function ProtectedRoute() {
  const { session, probing, sidebarOpen, shellTabs } = useContext(SessionContext);
  if (probing) return <CircularProgress sx={{ m: 4 }} />;
  if (!session) return <Navigate to="/login" replace />;
  return (
    <>
      <Navbar />
      <Toolbar variant="dense" />
      <Box sx={{ display: "flex", height: "calc(100vh - 48px)" }}>
        {shellTabs.length > 0 && <ShellsSidebar />}
        <Box component="main" sx={{ flexGrow: 1, overflow: "auto" }}>
          <Outlet />
        </Box>
        {sidebarOpen && <EventsSidebar />}
      </Box>
    </>
  );
}
