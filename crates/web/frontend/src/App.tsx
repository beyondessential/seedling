import { createBrowserRouter, RouterProvider } from "react-router-dom";
import ErrorPage from "./components/ErrorPage";
import { ProtectedRoute } from "./components/ProtectedRoute";
import { SessionProvider } from "./components/SessionProvider";
import AppDetail from "./routes/AppDetail";
import Apps from "./routes/Apps";
import CreateApp from "./routes/CreateApp";
import EditScript from "./routes/EditScript";
import Login from "./routes/Login";

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
