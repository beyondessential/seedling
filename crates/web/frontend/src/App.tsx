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
import Keys from "./routes/Keys";
import Login from "./routes/Login";
import Logs from "./routes/Logs";
import Registries from "./routes/Registries";
import Shell from "./routes/Shell";
import TemplateDetail from "./routes/TemplateDetail";
import Templates from "./routes/Templates";
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
      { path: "templates", element: <Templates /> },
      { path: "templates/:name", element: <TemplateDetail /> },
      { path: "faults", element: <Faults /> },
      { path: "volumes", element: <Volumes /> },
      { path: "backups", element: <Backups /> },
      { path: "keys", element: <Keys /> },
      { path: "registries", element: <Registries /> },
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
