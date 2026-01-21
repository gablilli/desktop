import {
  Box,
  Card,
  CardContent,
  Typography,
  IconButton,
  LinearProgress,
  Button,
  Stack,
  Tooltip,
  Link,
} from "@mui/material";
import {
  Delete as DeleteIcon,
  Refresh as RefreshIcon,
  FolderOpen as FolderOpenIcon,
  Language as LanguageIcon,
} from "@mui/icons-material";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { DriveInfo } from "./types";

interface DriveInfoResponse {
  id: string;
  name: string;
  instance_url: string;
  sync_path: string;
  icon_path?: string;
  raw_icon_path?: string;
  remote_path: string;
  enabled: boolean;
  user_id: string;
  status: string;
  capacity?: {
    total: number;
    used: number;
    label: string;
  };
}

export default function DrivesSection() {
  const { t } = useTranslation();
  const [drives, setDrives] = useState<DriveInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const isFetchingRef = useRef(false);

  const fetchDrives = useCallback(async () => {
    if (isFetchingRef.current) return;

    isFetchingRef.current = true;
    try {
      const result = await invoke<DriveInfoResponse[]>("get_drives_info");
      setDrives(
        result.map((drive) => ({
          ...drive,
          status: drive.status as DriveInfo["status"],
        }))
      );
    } catch (error) {
      console.error("Failed to fetch drives:", error);
    } finally {
      isFetchingRef.current = false;
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchDrives();
  }, [fetchDrives]);

  const handleDelete = async (driveId: string) => {
    try {
      await invoke("remove_drive", { driveId });
      await fetchDrives();
    } catch (error) {
      console.error("Failed to delete drive:", error);
    }
  };

  const handleReauthorize = async (drive: DriveInfo) => {
    try {
      const authUrl = `${drive.instance_url}/session/authorize`;
      await openUrl(authUrl);
    } catch (error) {
      console.error("Failed to open auth URL:", error);
    }
  };

  const handleOpenFolder = async (path: string) => {
    try {
      await invoke("show_file_in_explorer", { path });
    } catch (error) {
      console.error("Failed to open folder:", error);
    }
  };

  const handleOpenSite = async (url: string) => {
    try {
      await openUrl(url);
    } catch (error) {
      console.error("Failed to open site:", error);
    }
  };

  const getStatusColor = (status: DriveInfo["status"]) => {
    switch (status) {
      case "active":
        return "#4caf50"; // green
      case "event_push_lost":
        return "#ff9800"; // orange
      case "credential_expired":
        return "#f44336"; // red
      default:
        return "#9e9e9e"; // grey
    }
  };

  const getFolderName = (path: string) => {
    const parts = path.replace(/\\/g, "/").split("/");
    return parts[parts.length - 1] || path;
  };

  const getStatusLabel = (status: DriveInfo["status"]) => {
    switch (status) {
      case "active":
        return t("settings.driveStatus.active");
      case "event_push_lost":
        return t("settings.driveStatus.eventPushLost");
      case "credential_expired":
        return t("settings.driveStatus.credentialExpired");
      default:
        return status;
    }
  };

  if (loading) {
    return (
      <Box>
        <Typography variant="h5" fontWeight={500} gutterBottom>
          {t("settings.drives")}
        </Typography>
        <Typography variant="body2" color="text.secondary">
          {t("settings.loading")}
        </Typography>
      </Box>
    );
  }

  return (
    <Box>
      {drives.length === 0 ? (
        <Typography variant="body2" color="text.secondary">
          {t("settings.noDrives")}
        </Typography>
      ) : (
        <Stack spacing={2}>
          {drives.map((drive) => (
            <Card key={drive.id} variant="outlined">
              <CardContent sx={{pb:"16px!important"}}>
                <Box
                  sx={{
                    display: "flex",
                    alignItems: "flex-start",
                    gap: 2,
                  }}
                >
                  {/* Drive Icon */}
                  <Box
                    sx={{
                      width: 48,
                      height: 48,
                      borderRadius: 1,
                      overflow: "hidden",
                      flexShrink: 0,
                      bgcolor: "action.hover",
                      display: "flex",
                      alignItems: "center",
                      justifyContent: "center",
                    }}
                  >
                    {drive.raw_icon_path ? (
                      <img
                        src={convertFileSrc(drive.raw_icon_path)}
                        alt=""
                        style={{ width: 40, height: 40, objectFit: "contain" }}
                      />
                    ) : (
                      <FolderOpenIcon sx={{ fontSize: 32, color: "text.secondary" }} />
                    )}
                  </Box>

                  {/* Drive Info */}
                  <Box sx={{ flex: 1, minWidth: 0 }}>
                    {/* Name and Status */}
                    <Box
                      sx={{
                        display: "flex",
                        alignItems: "center",
                        gap: 1,
                        mb: 1,
                      }}
                    >
                      <Typography variant="subtitle1" fontWeight={500} noWrap>
                        {drive.name}
                      </Typography>
                      <Box
                        sx={{
                          display: "flex",
                          alignItems: "center",
                          gap: 0.5,
                        }}
                      >
                        <Box
                          sx={{
                            width: 8,
                            height: 8,
                            borderRadius: "50%",
                            bgcolor: getStatusColor(drive.status),
                          }}
                        />
                        <Typography
                          variant="body2"
                          sx={{ color: getStatusColor(drive.status) }}
                        >
                          {getStatusLabel(drive.status)}
                        </Typography>
                      </Box>
                    </Box>

                    {/* Site URL */}
                    <Tooltip title={drive.remote_path} placement="bottom-start">
                      <Box
                        sx={{
                          display: "flex",
                          alignItems: "center",
                          gap: 0.75,
                          mb: 0.5,
                        }}
                      >
                        <LanguageIcon
                          sx={{ fontSize: 16, color: "text.secondary" }}
                        />
                        <Link
                          component="button"
                          variant="body2"
                          color="text.secondary"
                          underline="hover"
                          onClick={() => handleOpenSite(drive.instance_url)}
                          sx={{
                            textAlign: "left",
                            overflow: "hidden",
                            textOverflow: "ellipsis",
                            whiteSpace: "nowrap",
                          }}
                        >
                          {drive.instance_url}
                        </Link>
                      </Box>
                    </Tooltip>

                    {/* Folder Path */}
                    <Tooltip title={drive.sync_path} placement="bottom-start">
                      <Box
                        sx={{
                          display: "flex",
                          alignItems: "center",
                          gap: 0.75,
                          mb: 1.5,
                        }}
                      >
                        <FolderOpenIcon
                          sx={{ fontSize: 16, color: "text.secondary" }}
                        />
                        <Link
                          component="button"
                          variant="body2"
                          color="text.secondary"
                          underline="hover"
                          onClick={() => handleOpenFolder(drive.sync_path)}
                          sx={{
                            textAlign: "left",
                            overflow: "hidden",
                            textOverflow: "ellipsis",
                            whiteSpace: "nowrap",
                          }}
                        >
                          {getFolderName(drive.sync_path)}
                        </Link>
                      </Box>
                    </Tooltip>

                    {/* Storage Usage */}
                    {drive.capacity && (
                      <Box sx={{ mb: 1 }}>
                        <Box
                          sx={{
                            display: "flex",
                            justifyContent: "space-between",
                            mb: 0.5,
                          }}
                        >
                          <Typography variant="caption" color="text.secondary">
                            {t("settings.storage")}
                          </Typography>
                          <Typography variant="caption" color="text.secondary">
                            {drive.capacity.label}
                          </Typography>
                        </Box>
                        <LinearProgress
                          variant="determinate"
                          value={
                            drive.capacity.total > 0
                              ? (drive.capacity.used / drive.capacity.total) * 100
                              : 0
                          }
                          sx={{ height: 6, borderRadius: 1 }}
                        />
                      </Box>
                    )}

                    {/* Action Buttons */}
                    <Box
                      sx={{
                        display: "flex",
                        alignItems: "center",
                        gap: 1,
                        mt: 1,
                      }}
                    >
                      {drive.status === "credential_expired" && (
                        <Button
                          size="small"
                          variant="outlined"
                          startIcon={<RefreshIcon />}
                          onClick={() => handleReauthorize(drive)}
                        >
                          {t("settings.reauthorize")}
                        </Button>
                      )}

                      <Box sx={{ flex: 1 }} />

                      <Tooltip title={t("settings.deleteDrive")}>
                        <IconButton
                          size="small"
                          color="error"
                          onClick={() => handleDelete(drive.id)}
                        >
                          <DeleteIcon fontSize="small" />
                        </IconButton>
                      </Tooltip>
                    </Box>
                  </Box>
                </Box>
              </CardContent>
            </Card>
          ))}
        </Stack>
      )}
    </Box>
  );
}
