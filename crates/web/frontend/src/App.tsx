import { createBrowserRouter, RouterProvider } from "react-router-dom";
import ErrorPage from "./components/ErrorPage";
import { ProtectedRoute } from "./components/ProtectedRoute";
import { SessionProvider } from "./components/SessionProvider";
import AppDetail from "./routes/AppDetail";
import Apps from "./routes/Apps";
import Backups from "./routes/Backups";
import CreateApp from "./routes/CreateApp";
import EditScript from "./routes/EditScript";
import Faults from "./routes/Faults";
import InfraLogs from "./routes/InfraLogs";
import Login from "./routes/Login";
import Logs from "./routes/Logs";
import Shell from "./routes/Shell";
import Volumes from "./routes/Volumes";

const router = createBrowserRouter([
  { path: "/login", element: <Login />, errorElement: <ErrorPage /> },
  {
    element: <ProtectedRoute />,
    errorElement: <ErrorPage />,
    children: [
      { index: true, element: <Apps /> },
      { path: "apps/new", element: <CreateApp /> },
      { path: "apps/:name", element: <AppDetail /> },
      { path: "apps/:name/script", element: <EditScript /> },
      { path: "faults", element: <Faults /> },
      { path: "volumes", element: <Volumes /> },
      { path: "backups", element: <Backups /> },
      { path: "apps/:name/logs", element: <Logs /> },
      { path: "apps/:name/shell/:shellName", element: <Shell /> },
      { path: "infra/:component/logs", element: <InfraLogs /> },
    ],
  },
]);

export default function App() {
  return (
    <SessionProvider>
      <RouterProvider router={router} />
    </SessionProvider>
  );
}
