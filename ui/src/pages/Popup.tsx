import {
  Box,
  Chip,
  IconButton,
  LinearProgress,
  List,
  ListItem,
  ListItemIcon,
  ListItemText,
  Typography,
  Divider,
  Stack,
  Skeleton,
} from "@mui/material";
import {
  Settings as SettingsIcon,
  Add as AddIcon,
  InsertDriveFile as DefaultFileIcon,
  Folder as FolderIcon,
  CheckCircle as CheckCircleIcon,
  Error as ErrorIcon,
  CloudUpload as UploadIcon,
  CloudDownload as DownloadIcon,
} from "@mui/icons-material";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useTranslation } from "react-i18next";
import { useTheme } from "@mui/material/styles";
import logoDark from "../assets/logo.svg";
import logoLight from "../assets/logo_light.svg";

interface DriveConfig {
  id: string;
  name: string;
  instance_url: string;
  sync_path: string;
  icon_path?: string;
}

interface TaskProgress {
  task_id: string;
  kind: "Upload" | "Download";
  local_path: string;
  progress: number;
  processed_bytes?: number;
  total_bytes?: number;
  speed_bytes_per_sec: number;
  eta_seconds?: number;
}

interface TaskRecord {
  id: string;
  drive_id: string;
  task_type: string;
  local_path: string;
  status: "Pending" | "Running" | "Completed" | "Failed" | "Cancelled";
  progress: number;
  total_bytes: number;
  processed_bytes: number;
  error?: string;
  created_at: number;
  updated_at: number;
}

interface TaskWithProgress extends TaskRecord {
  live_progress?: TaskProgress;
}

interface StatusSummary {
  drives: DriveConfig[];
  active_tasks: TaskWithProgress[];
  finished_tasks: TaskRecord[];
}

interface FileIconResponse {
  data: string; // Base64 encoded RGBA pixel data
  width: number;
  height: number;
}

// Global cache for file icons to avoid repeated fetches
const iconCache = new Map<string, string>();

function formatBytes(bytes: number): string {
  if (bytes === 0) return "0 B";
  const k = 1024;
  const sizes = ["B", "KB", "MB", "GB"];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + " " + sizes[i];
}

function formatRelativeTime(timestamp: number): string {
  const now = Date.now() / 1000;
  const diff = now - timestamp;

  if (diff < 60) return "Just now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return `${Math.floor(diff / 86400)}d ago`;
}

function getFileName(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

// Convert base64 RGBA data to a data URL using canvas
function rgbaToDataUrl(
  base64Data: string,
  width: number,
  height: number
): string {
  const binaryString = atob(base64Data);
  const bytes = new Uint8Array(binaryString.length);
  for (let i = 0; i < binaryString.length; i++) {
    bytes[i] = binaryString.charCodeAt(i);
  }

  const canvas = document.createElement("canvas");
  canvas.width = width;
  canvas.height = height;
  const ctx = canvas.getContext("2d");
  if (!ctx) return "";

  const imageData = ctx.createImageData(width, height);
  imageData.data.set(bytes);
  ctx.putImageData(imageData, 0, 0);

  return canvas.toDataURL("image/png");
}

interface FileIconProps {
  path: string;
  size?: number;
}

function FileIcon({ path, size = 24 }: FileIconProps) {
  const [iconUrl, setIconUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(false);
  const [isVisible, setIsVisible] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  // Get cache key based on path extension (icons are same for same file types)
  const cacheKey = useMemo(() => {
    const ext = path.split(".").pop()?.toLowerCase() || "unknown";
    return `${ext}_${size}`;
  }, [path, size]);

  // Intersection Observer for lazy loading
  useEffect(() => {
    const element = containerRef.current;
    if (!element) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0].isIntersecting) {
          setIsVisible(true);
          observer.disconnect();
        }
      },
      { threshold: 0.1 }
    );

    observer.observe(element);

    return () => {
      observer.disconnect();
    };
  }, []);

  // Fetch icon only when visible
  useEffect(() => {
    if (!isVisible) return;

    let mounted = true;

    const fetchIcon = async () => {
      // Check cache first
      const cached = iconCache.get(cacheKey);
      if (cached) {
        setIconUrl(cached);
        setLoading(false);
        return;
      }

      try {
        const response = await invoke<FileIconResponse>("get_file_icon", {
          path,
          size: 64,
        });

        if (!mounted) return;

        const dataUrl = rgbaToDataUrl(
          response.data,
          response.width,
          response.height
        );

        if (dataUrl) {
          iconCache.set(cacheKey, dataUrl);
          setIconUrl(dataUrl);
        } else {
          setError(true);
        }
      } catch (err) {
        console.error("Failed to fetch file icon:", err);
        if (mounted) {
          setError(true);
        }
      } finally {
        if (mounted) {
          setLoading(false);
        }
      }
    };

    fetchIcon();

    return () => {
      mounted = false;
    };
  }, [isVisible, path, size, cacheKey]);

  // Show placeholder until visible
  if (!isVisible) {
    return (
      <Box
        ref={containerRef}
        sx={{ width: size, height: size, display: "flex", alignItems: "center", justifyContent: "center" }}
      >
        <DefaultFileIcon sx={{ fontSize: size }} color="action" />
      </Box>
    );
  }

  if (loading) {
    return <Skeleton variant="rectangular" width={size} height={size} />;
  }

  if (error || !iconUrl) {
    return <DefaultFileIcon sx={{ fontSize: size }} color="action" />;
  }

  return (
    <Box
      component="img"
      src={iconUrl}
      alt="file icon"
      sx={{
        width: size,
        height: size,
        objectFit: "contain",
      }}
    />
  );
}

interface TaskItemProps {
  task: TaskWithProgress | TaskRecord;
  isActive?: boolean;
}

function TaskItem({ task, isActive = false }: TaskItemProps) {
  const activeTask = task as TaskWithProgress;
  const liveProgress = activeTask.live_progress;
  const progress = liveProgress?.progress ?? task.progress;
  const isUpload = task.task_type === "upload";
  const fileName = getFileName(task.local_path);

  const getStatusBadge = () => {
    if (isActive) {
      return isUpload ? (
        <UploadIcon sx={{ fontSize: 14 }} color="primary" />
      ) : (
        <DownloadIcon sx={{ fontSize: 14 }} color="primary" />
      );
    }
    switch (task.status) {
      case "Completed":
        return <CheckCircleIcon sx={{ fontSize: 14 }} color="success" />;
      case "Failed":
      case "Cancelled":
        return <ErrorIcon sx={{ fontSize: 14 }} color="error" />;
      default:
        return null;
    }
  };

  const getSecondaryText = () => {
    if (isActive && liveProgress) {
      const processed = formatBytes(liveProgress.processed_bytes ?? 0);
      const total = formatBytes(liveProgress.total_bytes ?? 0);
      const speed = formatBytes(liveProgress.speed_bytes_per_sec);
      return `${processed} / ${total} - ${speed}/s`;
    }
    if (isActive) {
      return task.status === "Pending" ? "Waiting..." : "Processing...";
    }
    return formatRelativeTime(task.updated_at);
  };

  const statusBadge = getStatusBadge();

  return (
    <ListItem
      sx={{
        px: 2,
        py: 1,
        "&:hover": {
          bgcolor: "action.hover",
          borderRadius: 1,
        },
      }}
    >
      <ListItemIcon sx={{ minWidth: 40 }}>
        <Box sx={{ position: "relative", width: 28, height: 28 }}>
          <FileIcon path={task.local_path} size={28} />
          {statusBadge && (
            <Box
              sx={{
                position: "absolute",
                bottom: -4,
                right: -4,
                bgcolor: "background.paper",
                borderRadius: "50%",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                width: 18,
                height: 18,
              }}
            >
              {statusBadge}
            </Box>
          )}
        </Box>
      </ListItemIcon>
      <ListItemText
        primary={
          <Typography variant="body2" noWrap sx={{ fontWeight: 500 }}>
            {fileName}
          </Typography>
        }
        secondary={
          <Box>
            <Typography variant="caption" color="text.secondary">
              {getSecondaryText()}
            </Typography>
            {isActive && (
              <LinearProgress
                variant="determinate"
                value={progress * 100}
                sx={{ mt: 0.5, height: 4, borderRadius: 2 }}
              />
            )}
          </Box>
        }
      />
    </ListItem>
  );
}

export default function Popup() {
  const { t } = useTranslation();
  const theme = useTheme();
  const [summary, setSummary] = useState<StatusSummary | null>(null);
  const [selectedDrive, setSelectedDrive] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const isFetchingRef = useRef(false);

  // Select logo based on theme mode
  const logo = theme.palette.mode === "dark" ? logoLight : logoDark;

  // Close window on blur (when it loses focus)
  useEffect(() => {
    let unlisten: () => void;
    const currentWindow = getCurrentWindow();

    currentWindow
      .onFocusChanged(({ payload: focused }) => {
        if (!focused) {
          currentWindow.close();
        }
      })
      .then((fn) => {
        unlisten = fn;
      });

    return () => {
      if (unlisten) {
        unlisten();
      }
    };
  }, []);

  // Fetch status summary
  const fetchSummary = useCallback(async () => {
    if (isFetchingRef.current) return;

    isFetchingRef.current = true;
    try {
      const result = await invoke<StatusSummary>("get_status_summary", {
        driveId: selectedDrive,
      });
      setSummary(result);
    } catch (error) {
      console.error("Failed to fetch status summary:", error);
    } finally {
      isFetchingRef.current = false;
      setLoading(false);
    }
  }, [selectedDrive]);

  // Initial fetch and polling
  useEffect(() => {
    fetchSummary();

    const intervalId = setInterval(() => {
      fetchSummary();
    }, 1000);

    return () => {
      clearInterval(intervalId);
    };
  }, [fetchSummary]);

  const handleDriveSelect = (driveId: string | null) => {
    setSelectedDrive(driveId);
  };

  const handleAddDrive = async () => {
    try {
      await invoke("show_add_drive_window");
    } catch {
      // Command might not exist, open via other means
      console.log("Opening add drive window");
    }
  };

  const handleSettings = () => {
    // TODO: Open settings
    console.log("Opening settings");
  };

  const hasActiveTasks =
    summary?.active_tasks && summary.active_tasks.length > 0;
  const hasFinishedTasks =
    summary?.finished_tasks && summary.finished_tasks.length > 0;

  return (
    <Box
      sx={{
        height: "100vh",
        display: "flex",
        flexDirection: "column",
        bgcolor: "background.paper",
        overflow: "hidden",
      }}
    >
      {/* Header */}
      <Box
        sx={{
          px: 2,
          pt: 1.5,
          pb: 1,
          borderBottom: 1,
          borderColor: "divider",
          backgroundColor: (theme) =>
          theme.palette.mode === "light" ? theme.palette.grey[100] : theme.palette.grey[900],
        }}
      >
        {/* Top row: Logo and Settings */}
        <Box
          sx={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            mb: 1.5,
          }}
        >
          <Box sx={{ display: "flex", alignItems: "center", gap: 1 }}>
            <Box
              component="img"
              src={logo}
              alt="Cloudreve"
              sx={{  height: 28 }}
            />
          </Box>
          <IconButton size="small" onClick={handleSettings}>
            <SettingsIcon fontSize="small" />
          </IconButton>
        </Box>

        {/* Drive filter chips */}
        <Stack
          direction="row"
          spacing={0.5}
          sx={{
            overflowX: "auto",
            pb: 0.5,
            "&::-webkit-scrollbar": { display: "none" },
          }}
        >
          <Chip
            label={t("popup.allDrives", "All")}
            size="small"
            variant={selectedDrive === null ? "filled" : "outlined"}
            onClick={() => handleDriveSelect(null)}
            sx={{ flexShrink: 0 }}
          />
          {summary?.drives.map((drive) => (
            <Chip
              key={drive.id}
              label={drive.name}
              size="small"
              variant={selectedDrive === drive.id ? "filled" : "outlined"}
              onClick={() => handleDriveSelect(drive.id)}
              sx={{ flexShrink: 0 }}
            />
          ))}
          <Chip
            icon={<AddIcon />}
            label={t("popup.newDrive", "New Drive")}
            size="small"
            variant="outlined"
            onClick={handleAddDrive}
            sx={{ flexShrink: 0 }}
          />
        </Stack>
      </Box>

      {/* Task List */}
      <Box sx={{ flex: 1, overflow: "auto" }}>
        {loading ? (
          <Box
            sx={{
              display: "flex",
              justifyContent: "center",
              alignItems: "center",
              height: "100%",
            }}
          >
            <Typography variant="body2" color="text.secondary">
              {t("popup.loading", "Loading...")}
            </Typography>
          </Box>
        ) : !hasActiveTasks && !hasFinishedTasks ? (
          <Box
            sx={{
              display: "flex",
              flexDirection: "column",
              justifyContent: "center",
              alignItems: "center",
              height: "100%",
              gap: 1,
            }}
          >
            <FolderIcon sx={{ fontSize: 48, color: "text.disabled" }} />
            <Typography variant="body2" color="text.secondary">
              {t("popup.noActivity", "No recent activity")}
            </Typography>
          </Box>
        ) : (
          <List disablePadding>
            {/* Active Tasks */}
            {hasActiveTasks && (
              <>
                <Typography
                  variant="caption"
                  color="text.secondary"
                  sx={{
                    px: 2,
                    py: 1,
                    display: "block",
                    fontWeight: 600,
                    textTransform: "uppercase",
                  }}
                >
                  {t("popup.syncing", "Syncing")}
                </Typography>
                {summary?.active_tasks.map((task) => (
                  <TaskItem key={task.id} task={task} isActive />
                ))}
              </>
            )}

            {/* Divider between active and finished */}
            {hasActiveTasks && hasFinishedTasks && (
              <Divider sx={{ my: 1 }} />
            )}

            {/* Finished Tasks */}
            {hasFinishedTasks && (
              <>
                <Typography
                  variant="caption"
                  color="text.secondary"
                  sx={{
                    px: 2,
                    py: 1,
                    display: "block",
                    fontWeight: 600,
                    textTransform: "uppercase",
                  }}
                >
                  {t("popup.recent", "Recent")}
                </Typography>
                {summary?.finished_tasks.map((task) => (
                  <TaskItem key={task.id} task={task} />
                ))}
              </>
            )}
          </List>
        )}
      </Box>

      {/* Footer Status */}
      <Box
        sx={{
          px: 2,
          py: 1,
          borderTop: 1,
          borderColor: "divider",
          display: "flex",
          alignItems: "center",
          gap: 1,
        }}
      >
        <CheckCircleIcon
          sx={{ fontSize: 18, color: hasActiveTasks ? "primary.main" : "success.main" }}
        />
        <Typography variant="caption" color="text.secondary">
          {hasActiveTasks
            ? t("popup.syncingStatus", "Syncing {{count}} file(s)...", {
                count: summary?.active_tasks.length ?? 0,
              })
            : t("popup.upToDate", "Your files are up to date")}
        </Typography>
      </Box>
    </Box>
  );
}
