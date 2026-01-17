import { Suspense } from "react";
import { ThemeProvider, CssBaseline, CircularProgress, Box } from "@mui/material";
import { BrowserRouter, Routes, Route } from "react-router-dom";
import "@fontsource/roboto/300.css";
import "@fontsource/roboto/400.css";
import "@fontsource/roboto/500.css";
import "@fontsource/roboto/700.css";
import "./i18n";
import { theme } from "./theme";
import AddDrive from "./pages/AddDrive";

function LoadingFallback() {
  return (
    <Box
      sx={{
        display: "flex",
        justifyContent: "center",
        alignItems: "center",
        minHeight: "100vh",
      }}
    >
      <CircularProgress />
    </Box>
  );
}

function App() {
  return (
    <Suspense fallback={<LoadingFallback />}>
      <ThemeProvider theme={theme}>
        <CssBaseline />
        <BrowserRouter>
          <Routes>
            <Route path="/" element={<AddDrive />} />
            <Route path="/add-drive" element={<AddDrive />} />
          </Routes>
        </BrowserRouter>
      </ThemeProvider>
    </Suspense>
  );
}

export default App;
