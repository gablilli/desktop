import { Suspense, useMemo } from "react";
import {
  ThemeProvider,
  CssBaseline,
  Box,
  useMediaQuery,
} from "@mui/material";
import { Routes, Route, HashRouter } from "react-router-dom";
import "@fontsource/roboto/300.css";
import "@fontsource/roboto/400.css";
import "@fontsource/roboto/500.css";
import "@fontsource/roboto/700.css";
import "./i18n";
import { createAppTheme } from "./theme";
import AddDrive from "./pages/AddDrive";
import Popup from "./pages/Popup";

function LoadingFallback() {
  return (
    <Box
      sx={{
        display: "flex",
        justifyContent: "center",
        alignItems: "center",
        height:"80vh"
      }}
    >
    </Box>
  );
}

function App() {
  const prefersDarkMode = useMediaQuery("(prefers-color-scheme: dark)");
  const theme = useMemo(
    () => createAppTheme(prefersDarkMode ? "dark" : "light"),
    [prefersDarkMode]
  );

  return (
    <Suspense fallback={<LoadingFallback />}>
      <ThemeProvider theme={theme}>
        <CssBaseline />
        <HashRouter>
          <Routes>
            <Route path="/add-drive" element={<AddDrive />} />
            <Route path="/popup" element={<Popup />} />
          </Routes>
        </HashRouter>
      </ThemeProvider>
    </Suspense>
  );
}

export default App;
