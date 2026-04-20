import { createTheme } from "@mui/material/styles";

export function makeTheme(mode: "light" | "dark") {
  return createTheme({
    palette: {
      mode,
      primary: { main: mode === "dark" ? "#4caf50" : "#2e7d32" },
      secondary: { main: mode === "dark" ? "#a1887f" : "#6d4c41" },
    },
  });
}
