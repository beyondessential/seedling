import { createBrowserRouter, RouterProvider } from "react-router-dom";
import ErrorPage from "./components/ErrorPage";
import { ProtectedRoute } from "./components/ProtectedRoute";
import { SessionProvider } from "./components/SessionProvider";
import AppDetail from "./routes/AppDetail";
import Apps from "./routes/Apps";
import Login from "./routes/Login";

const router = createBrowserRouter([
  { path: "/login", element: <Login />, errorElement: <ErrorPage /> },
  {
    element: <ProtectedRoute />,
    errorElement: <ErrorPage />,
    children: [
      { index: true, element: <Apps /> },
      { path: "apps/:name", element: <AppDetail /> },
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
