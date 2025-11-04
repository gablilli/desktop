# Architecture Overview

## System Design

```
┌─────────────────────────────────────────────────────────────┐
│                        GUI Service                          │
│                   (Your Frontend App)                       │
└────────────┬────────────────────────────────┬───────────────┘
             │                                │
             │ HTTP Requests                  │ SSE Events
             │ (Drive Management)             │ (Real-time Updates)
             │                                │
┌────────────▼────────────────────────────────▼───────────────┐
│                                                              │
│                    Cloudreve Sync Service                    │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐  │
│  │                    HTTP Server                        │  │
│  │                      (Axum)                          │  │
│  │                                                      │  │
│  │  ┌─────────────┐  ┌─────────────┐  ┌────────────┐  │  │
│  │  │   REST API  │  │  SSE Route  │  │  Health    │  │  │
│  │  │  Endpoints  │  │  /api/events│  │  /health   │  │  │
│  │  └──────┬──────┘  └──────┬──────┘  └────────────┘  │  │
│  └─────────┼─────────────────┼──────────────────────────┘  │
│            │                 │                              │
│            │                 │                              │
│  ┌─────────▼─────────┐  ┌───▼──────────────────┐          │
│  │                   │  │                       │          │
│  │  DriveManager     │  │  EventBroadcaster     │          │
│  │                   │  │                       │          │
│  │  • Add/Remove     │  │  • Tokio Broadcast    │          │
│  │  • Update         │  │  • Multiple Subs      │          │
│  │  • List           │  │  • Event Types        │          │
│  │  • Persist        │  │  • Helper Methods     │          │
│  │                   │  │                       │          │
│  └─────────┬─────────┘  └───────────────────────┘          │
│            │                                                │
│  ┌─────────▼─────────┐                                     │
│  │                   │                                     │
│  │  ~/.cloudreve/    │                                     │
│  │  drives.json      │                                     │
│  │                   │                                     │
│  └───────────────────┘                                     │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

## Component Responsibilities

### 1. Main Application (`main.rs`)

- **Initialization**: Sets up logging, creates managers, starts server
- **Lifecycle Management**: Handles startup and graceful shutdown
- **Signal Handling**: Responds to Ctrl+C and SIGTERM
- **State Persistence**: Ensures configs are saved before exit

### 2. HTTP API (`api/mod.rs`)

- **Routing**: Defines all HTTP endpoints
- **Request Handling**: Processes REST API calls
- **Error Handling**: Provides structured error responses
- **SSE Streaming**: Manages Server-Sent Events connections

**Endpoints:**

- `GET /health` - Health check
- `GET /api/drives` - List all drives
- `POST /api/drives` - Add new drive
- `GET /api/drives/:id` - Get specific drive
- `PUT /api/drives/:id` - Update drive
- `DELETE /api/drives/:id` - Remove drive
- `POST /api/drives/:id/sync` - Start/stop sync
- `GET /api/drives/:id/status` - Get sync status
- `GET /api/events` - SSE event stream

### 3. Drive Manager (`drive/mod.rs`)

- **State Management**: In-memory drive configurations with RwLock
- **Persistence**: JSON-based storage in `~/.cloudreve/drives.json`
- **CRUD Operations**: Full drive lifecycle management
- **Sync Operations**: Placeholder methods for actual sync implementation

**Key Methods:**

```rust
// Lifecycle
pub fn new() -> Result<Self>
pub async fn load() -> Result<()>
pub async fn persist() -> Result<()>

// CRUD
pub async fn add_drive(&self, config: DriveConfig) -> Result<String>
pub async fn remove_drive(&self, id: &str) -> Result<Option<DriveConfig>>
pub async fn get_drive(&self, id: &str) -> Option<DriveConfig>
pub async fn list_drives(&self) -> Vec<DriveConfig>
pub async fn update_drive(&self, id: &str, config: DriveConfig) -> Result<()>

// Sync (Placeholders)
pub async fn start_sync(&self, id: &str) -> Result<()>
pub async fn stop_sync(&self, id: &str) -> Result<()>
pub async fn get_sync_status(&self, id: &str) -> Result<serde_json::Value>
```

### 4. Event Broadcaster (`events/mod.rs`)

- **Event Distribution**: Tokio broadcast channel for pub-sub pattern
- **Type Safety**: Strongly-typed event enum
- **Helper Methods**: Convenient functions for common events
- **Subscriber Management**: Automatic cleanup of disconnected clients

**Event Types:**

```rust
pub enum Event {
    DriveAdded { drive_id: String, name: String },
    DriveRemoved { drive_id: String },
    DriveUpdated { drive_id: String },
    SyncStarted { drive_id: String },
    SyncCompleted { drive_id: String, files_synced: u64 },
    SyncProgress { drive_id: String, progress: f32, current_file: String },
    SyncError { drive_id: String, error: String },
    FileUploaded { drive_id: String, file_path: String },
    FileDownloaded { drive_id: String, file_path: String },
    ConnectionStatusChanged { connected: bool },
    Custom { event_name: String, payload: serde_json::Value },
}
```

## Data Flow

### Adding a Drive

```
1. GUI → POST /api/drives (JSON body)
2. API Handler validates request
3. API Handler → DriveManager.add_drive()
4. DriveManager updates in-memory state
5. DriveManager.persist() saves to disk
6. EventBroadcaster.drive_added() sends event
7. SSE clients receive DriveAdded event
8. API responds with created drive
```

### Starting Sync

```
1. GUI → POST /api/drives/:id/sync {"action": "start"}
2. API Handler → DriveManager.start_sync()
3. [Your sync implementation runs here]
4. EventBroadcaster.sync_started() sends event
5. Periodic progress updates via EventBroadcaster.sync_progress()
6. On completion: EventBroadcaster.sync_completed()
7. All SSE clients receive real-time updates
```

### SSE Connection

```
1. GUI → GET /api/events (establishes SSE connection)
2. API creates broadcast receiver
3. Receiver → Event Stream → SSE Response
4. Any event broadcast goes to all connected clients
5. Connection maintained with keep-alive
6. Auto-reconnect on disconnect
```

## Thread Safety

- **DriveManager**: Uses `Arc<RwLock<DriveState>>` for concurrent access
- **EventBroadcaster**: Uses `Arc<broadcast::Sender<Event>>` for thread-safe broadcasting
- **AppState**: Both managers are wrapped in Arc and cloned per request

## Error Handling

```rust
pub enum AppError {
    NotFound(String),      // 404
    BadRequest(String),    // 400
    Internal(anyhow::Error), // 500
}
```

All errors are converted to structured JSON responses:

```json
{
  "success": false,
  "data": null,
  "error": "Error message here"
}
```

## Persistence Strategy

- **Format**: JSON for human readability and easy debugging
- **Location**: `~/.cloudreve/drives.json` (platform-agnostic via `dirs` crate)
- **Timing**:
  - On startup: Load existing configs
  - On modification: Immediate persist
  - On shutdown: Final persist (graceful)
- **Atomicity**: File writes are not atomic - consider using tempfile + rename for production

## Scalability Considerations

### Current Implementation

- Single process
- In-memory state
- File-based persistence
- Broadcast channel (bounded queue)

### For Production

1. **Database**: Replace JSON files with SQLite/PostgreSQL
2. **Distributed Events**: Use Redis pub-sub or message queue
3. **Multiple Instances**: Add service discovery and load balancing
4. **State Recovery**: Implement WAL (Write-Ahead Logging)
5. **Rate Limiting**: Add per-client rate limits for API

## Extension Points

### Adding Custom Sync Logic

Implement these placeholder methods in `DriveManager`:

```rust
pub async fn start_sync(&self, id: &str) -> Result<()> {
    // 1. Get drive config
    // 2. Initialize sync worker
    // 3. Start file watcher
    // 4. Broadcast progress events
    // 5. Handle errors
}
```

### Adding Custom Events

```rust
// Define new event variant
pub enum Event {
    // ... existing variants
    YourCustomEvent { field1: String, field2: i32 },
}

// Add helper method
impl EventBroadcaster {
    pub fn your_custom_event(&self, field1: String, field2: i32) {
        self.broadcast(Event::YourCustomEvent { field1, field2 });
    }
}
```

### Adding New API Endpoints

```rust
// In api/mod.rs
pub fn create_router(state: AppState) -> Router {
    Router::new()
        // ... existing routes
        .route("/api/your-endpoint", get(your_handler))
        .with_state(state)
}

async fn your_handler(
    State(state): State<AppState>,
) -> Result<Json<ApiResponse<YourType>>, AppError> {
    // Your logic here
}
```

## Security Considerations

**Current Implementation** (development):

- No authentication
- No authorization
- No encryption
- Localhost binding only

**For Production**:

1. Add API key or OAuth authentication
2. Implement user-based drive ownership
3. Use TLS/HTTPS
4. Validate and sanitize all inputs
5. Add rate limiting
6. Implement audit logging
