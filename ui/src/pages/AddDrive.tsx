import { Box, Button, Container, Snackbar, Typography } from "@mui/material";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import defaultLogo from "../assets/cloudreve.svg";
import { FilledTextField } from "../common/StyledComponent";
import { fetchSiteIcon, isValidUrl } from "../utils/manifest";
import { generatePKCEPair } from "../utils/pkce";
import {
  isValidationError,
  validateSiteVersion,
} from "../utils/siteValidation";

// Store PKCE data for use after OAuth redirect
export interface PKCESession {
  codeVerifier: string;
  codeChallenge: string;
  siteUrl: string;
  siteVersion: string;
  siteIcon?: string;
}

// React ref to store PKCE session data
let pkceSessionRef: PKCESession | null = null;

export function getPKCESession(): PKCESession | null {
  return pkceSessionRef;
}

export function clearPKCESession(): void {
  pkceSessionRef = null;
}

function buildAuthorizeUrl(siteUrl: string, codeChallenge: string): string {
  const url = new URL("/session/authorize", siteUrl);
  const params = {
    response_type: "code",
    client_id: "393a1839-f52e-498e-9972-e77cc2241eee",
    scope: "profile email openid offline_access UserInfo.Write Workflow.Write Files.Write Shares.Write",
    redirect_uri: "/desktopCallback",
    code_challenge: codeChallenge,
    code_challenge_method: "S256",
  };
  // Use encodeURIComponent to encode spaces as %20 instead of +
  url.search = Object.entries(params)
    .map(([key, value]) => `${encodeURIComponent(key)}=${encodeURIComponent(value)}`)
    .join("&");
  return url.toString();
}

export default function AddDrive() {
  const { t } = useTranslation();
  const [siteUrl, setSiteUrl] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const [snackbarOpen, setSnackbarOpen] = useState(false);
  const [logo, setLogo] = useState(defaultLogo);
  const [authorizeUrl, setAuthorizeUrl] = useState<string | null>(null);
  const lastFetchedUrl = useRef<string>("");
  const currentIconUrl = useRef<string | undefined>(undefined);

  // Fetch site icon when URL changes and is valid
  const handleUrlBlur = () => {
    const trimmedUrl = siteUrl.trim();
    if (!isValidUrl(trimmedUrl) || trimmedUrl === lastFetchedUrl.current) {
      return;
    }

    lastFetchedUrl.current = trimmedUrl;

    fetchSiteIcon(trimmedUrl)
      .then((iconUrl) => {
        if (iconUrl) {
          // Preload the image to ensure it loads successfully
          const img = new Image();
          img.onload = () => {
            setLogo(iconUrl);
            currentIconUrl.current = iconUrl;
          };
          img.onerror = () => {
            console.error("Failed to load site icon:", iconUrl);
            currentIconUrl.current = undefined;
          };
          img.src = iconUrl;
        }
      })
      .catch((err) => {
        console.error("Failed to fetch manifest:", err);
        currentIconUrl.current = undefined;
      });
  };

  // Reset logo when URL is cleared or becomes invalid
  useEffect(() => {
    if (!siteUrl.trim() || !isValidUrl(siteUrl.trim())) {
      setLogo(defaultLogo);
      lastFetchedUrl.current = "";
      currentIconUrl.current = undefined;
    }
  }, [siteUrl]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setLoading(true);
    setSnackbarOpen(false);

    try {
      // Validate site version first
      const version = await validateSiteVersion(siteUrl);
      console.log("Site version:", version);

      // Generate PKCE pair
      const { codeVerifier, codeChallenge } = await generatePKCEPair();

      // Store PKCE session data for use after OAuth redirect
      pkceSessionRef = {
        codeVerifier,
        codeChallenge,
        siteUrl: siteUrl.trim(),
        siteVersion: version,
        siteIcon: currentIconUrl.current,
      };

      // Build and open the authorization URL
      const authUrl = buildAuthorizeUrl(siteUrl.trim(), codeChallenge);
      setAuthorizeUrl(authUrl);
      await openUrl(authUrl);
    } catch (error) {
      if (isValidationError(error)) {
        setError(t(`addDrive.errors.${error.type}`, error.params));
      } else {
        const message = error instanceof Error ? error.message : String(error);
        setError(t("addDrive.errors.connectionFailed", { message }));
      }
      setSnackbarOpen(true);
    } finally {
      setLoading(false);
    }
  };

  const handleOpenAuthorizeUrl = async () => {
    if (authorizeUrl) {
      await openUrl(authorizeUrl);
    }
  };

  const handleBack = () => {
    setAuthorizeUrl(null);
    pkceSessionRef = null;
  };

  const handleCloseSnackbar = () => {
    setSnackbarOpen(false);
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
        <Box
          sx={{
            p: 4,
            width: "100%",
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            gap: 1,
            borderRadius: 3,
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

          {authorizeUrl ? (
            // Waiting for sign-in state
            <>
              <Typography
                sx={{ mt: 2 }}
                variant="h5"
                component="h1"
                fontWeight={500}
              >
                {t("addDrive.waitingTitle")}
              </Typography>

              <Typography
                variant="body2"
                color="text.secondary"
                textAlign="center"
              >
                {t("addDrive.waitingDescription")}
              </Typography>

              <Box
                sx={{
                  width: "100%",
                  display: "flex",
                  flexDirection: "column",
                  gap: 2,
                  mt: 2,
                }}
              >
                <Button
                  variant="contained"
                  size="large"
                  fullWidth
                  onClick={handleOpenAuthorizeUrl}
                >
                  {t("addDrive.reopenBrowser")}
                </Button>

                <Button
                  variant="text"
                  size="large"
                  fullWidth
                  onClick={handleBack}
                >
                  {t("addDrive.back")}
                </Button>
              </Box>
            </>
          ) : (
            // Initial URL input state
            <>
              <Typography
                sx={{ mt: 2 }}
                variant="h5"
                component="h1"
                fontWeight={500}
              >
                {t("addDrive.title")}
              </Typography>

              <Typography
                variant="body2"
                color="text.secondary"
                textAlign="center"
              >
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
                  mt: 2,
                }}
              >
                <FilledTextField
                  fullWidth
                  autoComplete="off"
                  slotProps={{
                    input: {
                      readOnly: loading,
                    },
                  }}
                  label={t("addDrive.siteUrl")}
                  placeholder={t("addDrive.siteUrlPlaceholder")}
                  value={siteUrl}
                  onChange={(e) => setSiteUrl(e.target.value)}
                  onBlur={handleUrlBlur}
                  variant="filled"
                  type="url"
                  required
                />

                <Button
                  type="submit"
                  variant="contained"
                  size="large"
                  loading={loading}
                  fullWidth
                >
                  {t("addDrive.connect")}
                </Button>
              </Box>
            </>
          )}
        </Box>
      </Box>
      <Snackbar
        open={snackbarOpen}
        autoHideDuration={6000}
        onClose={handleCloseSnackbar}
        message={error}
      />
    </Container>
  );
}
