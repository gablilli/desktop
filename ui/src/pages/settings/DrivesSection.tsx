import {
  Box,
  Card,
  CardContent,
  Typography,
  LinearProgress,
  Stack,
  Tooltip,
  Link,
  Divider,
  FormControlLabel,
  Switch,
} from "@mui/material";
import {
  FolderOpen as FolderOpenIcon,
  LanguageRounded,
  FolderOpenRounded,
  Add as AddIcon,
  DeleteOutlineRounded,
  RefreshRounded,
} from "@mui/icons-material";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { convertFileSrc } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { DriveInfo } from "./types";
import {  SecondaryButton, SecondaryErrorButton } from "../../common/StyledComponent";
import { ask } from '@tauri-apps/plugin-dialog';

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
  const [syncDirections, setSyncDirections] = useState<Record<string, string>>({});

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
      
      // Fetch sync directions for all drives concurrently
      const directionPromises = result.map(async (drive) => {
        try {
          const direction = await invoke<string>("get_sync_direction", { driveId: drive.id });
          return { id: drive.id, direction };
        } catch (error) {
          console.error(`Failed to fetch sync direction for drive ${drive.id}:`, error);
          return { id: drive.id, direction: "two_way" }; // default
        }
      });
      
      const directionResults = await Promise.all(directionPromises);
      const directions: Record<string, string> = {};
      for (const { id, direction } of directionResults) {
        directions[id] = direction;
      }
      setSyncDirections(directions);
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

  const handleDelete = async (driveId: string, driveName: string) => {
    const confirmed = await ask(t("settings.deleteDriveConfirm", { name: driveName }), {
      title: t("settings.deleteDrive"),
      kind: "warning",
    });

    if (!confirmed) return;

    try {
      await invoke("remove_drive", { driveId });
      await fetchDrives();
    } catch (error) {
      console.error("Failed to delete drive:", error);
    }
  };

  const handleReauthorize = async (drive: DriveInfo) => {
    try {
      await invoke("show_reauthorize_window", {
        driveId: drive.id,
        siteUrl: drive.instance_url,
        driveName: drive.name,
      });
    } catch (error) {
      console.error("Failed to open reauthorize window:", error);
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

  const handleAddDrive = async () => {
    try {
      await invoke("show_add_drive_window");
    } catch (error) {
      console.error("Failed to open add drive window:", error);
    }
  };

  const handleSyncDirectionChange = async (driveId: string, checked: boolean) => {
    const newDirection = checked ? "one_way_upload" : "two_way";
    const previousDirection = syncDirections[driveId] || "two_way";
    
    // Optimistically update state
    setSyncDirections(prev => ({ ...prev, [driveId]: newDirection }));
    
    try {
      await invoke("set_sync_direction", { driveId, direction: newDirection });
    } catch (error) {
      console.error("Failed to change sync direction:", error);
      // Revert on error
      setSyncDirections(prev => ({ ...prev, [driveId]: previousDirection }));
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
              <CardContent sx={{ pb: "16px!important" }}>
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
                        justifyContent: "space-between",
                        gap: 1,
                        mb: 1,
                      }}
                    >
                      <Typography variant="body1" fontWeight={600} noWrap>
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
                          variant="caption"
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
                        <LanguageRounded
                          sx={{ fontSize: 16, color: "text.secondary" }}
                        />
                        <Link
                          component="button"
                          variant="caption"
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
                        <FolderOpenRounded
                          sx={{ fontSize: 16, color: "text.secondary" }}
                        />
                        <Link
                          component="button"
                          variant="caption"
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

                    {/* Sync Direction Toggle */}
                    <Tooltip title={t("settings.syncDirectionDescription")} placement="bottom-start">
                      <FormControlLabel
                        control={
                          <Switch
                            size="small"
                            checked={syncDirections[drive.id] === "one_way_upload"}
                            onChange={(e) => handleSyncDirectionChange(drive.id, e.target.checked)}
                            disabled={loading}
                          />
                        }
                        label={
                          <Typography variant="caption" color="text.secondary">
                            {t("settings.oneWaySync")}
                          </Typography>
                        }
                        sx={{ ml: 0 }}
                      />
                    </Tooltip>

                  </Box>
                </Box>

                {/* Action Footer */}
                <Divider sx={{ my: 2, mx: -2 }} />
                <Box
                  sx={{
                    display: "flex",
                    alignItems: "center",
                    gap: 1,
                  }}
                >
                  {drive.status === "credential_expired" && (
                    <SecondaryButton
                      size="small"
                      startIcon={<RefreshRounded />}
                      onClick={() => handleReauthorize(drive)}
                    >
                      {t("settings.reauthorize")}
                    </SecondaryButton>
                  )}

                  <Box sx={{ flex: 1 }} />

                  <SecondaryErrorButton
                    size="small"
                    color="error"
                    startIcon={<DeleteOutlineRounded />}
                    onClick={() => handleDelete(drive.id, drive.name)}
                  >
                    {t("settings.deleteDrive")}
                  </SecondaryErrorButton>
                </Box>
              </CardContent>
            </Card>
          ))}
        </Stack>
      )}

      <SecondaryButton
        startIcon={<AddIcon />}
        onClick={handleAddDrive}
        sx={{ mt: 2 }}
      >
        {t("popup.newDrive")}
      </SecondaryButton>
    </Box>
  );
}
