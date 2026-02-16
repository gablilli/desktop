# Drive Start Error Messages

## Overview

This document describes the improved error messages shown to users when a drive fails to start.

## Previous Behavior

Before this fix, all drive start failures showed the same generic error:
```
Failed to start drive
```

This was unhelpful as it didn't tell users:
- What actually went wrong
- How to fix the problem
- Where the problem occurred

## New Behavior

Now, users see specific error messages that include:
- The actual root cause of the failure
- The specific path or component that failed
- Actionable information when possible

## Error Messages by Failure Point

### 1. Cloud Filter API Availability Check Failed

**When**: Unable to check if Windows Cloud Filter API is available

**Error Message**:
```
Failed to check Windows Cloud Filter API availability
```

**Possible Causes**:
- Windows API system error
- Corrupted system files

**User Action**: 
- Restart the computer
- Run Windows System File Checker: `sfc /scannow`
- Contact support if the issue persists

---

### 2. Cloud Filter API Not Supported

**When**: Windows Cloud Filter API is not available on the system

**Error Message**:
```
Windows Cloud Filter API is not supported on this system. This feature requires Windows 10 version 1809 or later with the Cloud Files API enabled.
```

**User Action**: 
- Upgrade to Windows 10 version 1809 or later
- Ensure the Cloud Files API feature is enabled in Windows
- Verify that OneDrive or other cloud storage providers work correctly (they use the same API)

---

### 3. Sync Root ID Generation Failed

**When**: Unable to generate a unique identifier for the sync root

**Error Message**:
```
Failed to generate unique sync root identifier
```

**User Action**: This is a rare error. Try restarting the application.

---

### 4. Sync Directory Creation Failed

**When**: Unable to create the local sync directory

**Error Message**:
```
Failed to create sync directory at: C:\Users\Username\CloudreveDrive
```

**Possible Causes**:
- Path is too long (>260 characters on older Windows versions)
- Insufficient permissions
- Invalid characters in path
- Disk full

**User Action**: 
- Check if you have write permissions to the location
- Try a shorter path
- Ensure the drive has sufficient space

---

### 5. Recycle Bin URI Failed

**When**: Unable to set the recycle bin URI for the sync root

**Error Message**:
```
Failed to set recycle bin URI for sync root
```

**User Action**: This is typically a Windows API error. Contact support.

---

### 6. Sync Root Path Failed

**When**: Unable to set the sync root path

**Error Message**:
```
Failed to set sync root path: C:\Users\Username\CloudreveDrive
```

**Possible Causes**:
- Path doesn't exist
- Path is inaccessible
- Invalid path format

**User Action**: Ensure the directory exists and is accessible

---

### 7. Custom State Registration Failed

**When**: Unable to register custom file states with Windows

**Error Messages**:
```
Failed to add 'shared' custom state to sync root
```
or
```
Failed to add 'accessible' custom state to sync root
```

**Possible Causes**:
- Windows API issue
- Translation system not initialized (rare)
- COM registration problem

**User Action**: Try restarting the application. If problem persists, contact support.

---

### 8. Sync Root Registration Failed

**When**: Unable to register the sync root with Windows Cloud Filter API

**Error Message**:
```
Failed to register sync root with Windows Cloud Filter API
```

**Possible Causes**:
- Another sync root already exists at this location
- Sync root with same ID already registered
- Windows registry issues

**User Action**: 
- Try a different local path
- Unregister any existing sync roots at this location
- Restart Windows Explorer

---

### 9. Connection to Sync Root Failed

**When**: Unable to establish connection to the registered sync root

**Error Message**:
```
Failed to connect to sync root at: C:\Users\Username\CloudreveDrive
```

**Possible Causes**:
- Sync root not properly registered
- Windows Cloud Filter service not running
- Path became inaccessible

**User Action**:
- Ensure Windows Cloud Files service is running
- Check if path is still accessible
- Restart the application

---

### 10. File System Watcher Creation Failed

**When**: Unable to create the file system watcher

**Error Message**:
```
Failed to create file system watcher
```

**User Action**: This is a system resource issue. Restart the application.

---

### 11. File System Watch Start Failed

**When**: Unable to start watching the sync directory for changes

**Error Message**:
```
Failed to start watching file system at: C:\Users\Username\CloudreveDrive
```

**Possible Causes**:
- Too many file handles open
- System resource limits reached
- Path no longer accessible

**User Action**:
- Close other applications
- Restart the application
- Restart the computer

---

## Error Display

All these errors are:
1. Logged to the application logs with full stack traces
2. Displayed to the user in the UI (snackbar notification)
3. Include the full error chain from the underlying Windows API when applicable

## Technical Implementation

### Error Handling Flow

```
mount.start() error (specific context)
    ↓
add_drive() (preserves error as-is)
    ↓
Tauri command (converts to string)
    ↓
UI display (shows in snackbar)
```

### Key Files

- `/crates/cloudreve-sync/src/drive/mounts.rs` - Specific error contexts
- `/crates/cloudreve-sync/src/drive/manager/mod.rs` - Error propagation
- `/src-tauri/src/commands.rs` - Error to string conversion
- `/ui/src/pages/AddDrive.tsx` - UI error display
