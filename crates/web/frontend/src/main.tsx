import { CssBaseline, ThemeProvider, useMediaQuery } from "@mui/material";
import { StrictMode, useMemo } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { makeTheme } from "./theme";

function Root() {
  const prefersDark = useMediaQuery("(prefers-color-scheme: dark)");
  const theme = useMemo(() => makeTheme(prefersDark ? "dark" : "light"), [prefersDark]);
  return (
    <ThemeProvider theme={theme}>
      <CssBaseline />
      <App />
    </ThemeProvider>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Root />
  </StrictMode>,
);
