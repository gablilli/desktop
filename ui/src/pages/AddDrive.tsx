import {
  Box,
  Button,
  Container,
  Paper,
  TextField,
  Typography,
} from "@mui/material";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import logo from "../assets/cloudreve.svg";

export default function AddDrive() {
  const { t } = useTranslation();
  const [siteUrl, setSiteUrl] = useState("");

  const handleSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    // TODO: Handle site URL submission
    console.log("Site URL:", siteUrl);
  };

  return (
    <Container maxWidth="sm">
      <Box
        sx={{
          minHeight: "100vh",
          display: "flex",
          flexDirection: "column",
          justifyContent: "center",
          alignItems: "center",
          py: 4,
        }}
      >
        <Paper
          elevation={0}
          sx={{
            p: 4,
            width: "100%",
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            gap: 3,
            borderRadius: 3,
            bgcolor: "background.paper",
          }}
        >
          <Box
            component="img"
            src={logo}
            alt="Cloudreve"
            sx={{
              width: 120,
              height: "auto",
              mb: 2,
            }}
          />

          <Typography variant="h5" component="h1" fontWeight={500}>
            {t("addDrive.title")}
          </Typography>

          <Typography variant="body2" color="text.secondary" textAlign="center">
            {t("addDrive.description")}
          </Typography>

          <Box
            component="form"
            onSubmit={handleSubmit}
            sx={{
              width: "100%",
              display: "flex",
              flexDirection: "column",
              gap: 2,
            }}
          >
            <TextField
              fullWidth
              label={t("addDrive.siteUrl")}
              placeholder={t("addDrive.siteUrlPlaceholder")}
              value={siteUrl}
              onChange={(e) => setSiteUrl(e.target.value)}
              variant="filled"
              type="url"
              required
            />

            <Button
              type="submit"
              variant="contained"
              size="large"
              fullWidth
              sx={{ mt: 1 }}
            >
              {t("addDrive.connect")}
            </Button>
          </Box>
        </Paper>
      </Box>
    </Container>
  );
}
