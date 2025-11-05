use crate::client::{Client, RequestOptions};
use crate::error::ApiResult;
use crate::models::explorer::*;
use async_trait::async_trait;
use bytes::Bytes;

/// File explorer API methods
#[async_trait]
pub trait ExplorerApi {
    /// List files in a directory
    async fn list_files(&self, params: &ListFileService) -> ApiResult<ListResponse>;
    
    /// Get file thumbnail
    async fn get_file_thumb(&self, path: &str, context_hint: Option<&str>) -> ApiResult<FileThumbResponse>;
    
    /// Get file information
    async fn get_file_info(&self, params: &GetFileInfoService) -> ApiResult<FileResponse>;
    
    /// Create a new file or folder
    async fn create_file(&self, request: &CreateFileService) -> ApiResult<FileResponse>;
    
    /// Delete files
    async fn delete_files(&self, request: &DeleteFileService) -> ApiResult<()>;
    
    /// Rename a file
    async fn rename_file(&self, request: &RenameFileService) -> ApiResult<FileResponse>;
    
    /// Move files
    async fn move_files(&self, request: &MoveFileService) -> ApiResult<()>;
    
    /// Restore files from trash
    async fn restore_files(&self, request: &DeleteFileService) -> ApiResult<()>;
    
    /// Patch file metadata
    async fn patch_metadata(&self, request: &PatchMetadataService) -> ApiResult<()>;
    
    /// Get file entity URL
    async fn get_file_url(&self, request: &FileURLService) -> ApiResult<FileURLResponse>;
    
    /// Unlock files
    async fn unlock_files(&self, request: &UnlockFileService) -> ApiResult<()>;
    
    /// Set current version
    async fn set_current_version(&self, request: &VersionControlService) -> ApiResult<()>;
    
    /// Delete version
    async fn delete_version(&self, request: &VersionControlService) -> ApiResult<()>;
    
    /// Update file content
    async fn update_file(&self, params: &FileUpdateService, data: Bytes) -> ApiResult<FileResponse>;
    
    /// Get storage policy options
    async fn get_storage_policy_options(&self) -> ApiResult<Vec<StoragePolicy>>;
    
    /// Mount storage policy
    async fn mount_storage_policy(&self, request: &MountPolicyService) -> ApiResult<Vec<StoragePolicy>>;
    
    /// Set file permissions
    async fn set_permissions(&self, request: &SetPermissionService) -> ApiResult<()>;
    
    /// Create upload session
    async fn create_upload_session(&self, request: &UploadSessionRequest) -> ApiResult<UploadCredential>;
    
    /// Upload chunk
    async fn upload_chunk(
        &self,
        session_id: &str,
        chunk_index: usize,
        data: Bytes,
    ) -> ApiResult<UploadCredential>;
    
    /// Delete upload session
    async fn delete_upload_session(&self, request: &DeleteUploadSessionService) -> ApiResult<()>;
    
    /// Complete S3-like upload
    async fn complete_s3_upload(
        &self,
        policy_type: &str,
        session_id: &str,
        session_key: &str,
    ) -> ApiResult<()>;
    
    /// Complete OneDrive upload
    async fn complete_onedrive_upload(&self, session_id: &str, session_key: &str) -> ApiResult<()>;
}

#[async_trait]
impl ExplorerApi for Client {
    async fn list_files(&self, params: &ListFileService) -> ApiResult<ListResponse> {
        // Build query string
        let mut query_params = vec![format!("uri={}", urlencoding::encode(&params.uri))];
        
        if let Some(page) = params.page {
            query_params.push(format!("page={}", page));
        }
        if let Some(page_size) = params.page_size {
            query_params.push(format!("page_size={}", page_size));
        }
        if let Some(order_by) = &params.order_by {
            query_params.push(format!("order_by={}", order_by));
        }
        if let Some(order_direction) = &params.order_direction {
            query_params.push(format!("order_direction={}", order_direction));
        }
        if let Some(next_page_token) = &params.next_page_token {
            query_params.push(format!("next_page_token={}", next_page_token));
        }
        
        let query = format!("?{}", query_params.join("&"));
        
        self.get(
            &format!("/file{}", query),
            RequestOptions::new().with_purchase_ticket(),
        ).await
    }
    
    async fn get_file_thumb(&self, path: &str, _context_hint: Option<&str>) -> ApiResult<FileThumbResponse> {
        let query = format!("?uri={}", urlencoding::encode(path));
        
        // TODO: Add context hint header support if needed
        self.get(
            &format!("/file/thumb{}", query),
            RequestOptions::new().with_purchase_ticket(),
        ).await
    }
    
    async fn get_file_info(&self, params: &GetFileInfoService) -> ApiResult<FileResponse> {
        let mut query_params = vec![];
        
        if let Some(uri) = &params.uri {
            query_params.push(format!("uri={}", urlencoding::encode(uri)));
        }
        if let Some(id) = &params.id {
            query_params.push(format!("id={}", id));
        }
        if let Some(extended) = params.extended {
            query_params.push(format!("extended={}", extended));
        }
        if let Some(folder_summary) = params.folder_summary {
            query_params.push(format!("folder_summary={}", folder_summary));
        }
        
        let query = if query_params.is_empty() {
            String::new()
        } else {
            format!("?{}", query_params.join("&"))
        };
        
        self.get(
            &format!("/file/info{}", query),
            RequestOptions::new().with_purchase_ticket(),
        ).await
    }
    
    async fn create_file(&self, request: &CreateFileService) -> ApiResult<FileResponse> {
        self.post(
            "/file/create",
            request,
            RequestOptions::new().with_purchase_ticket(),
        ).await
    }
    
    async fn delete_files(&self, request: &DeleteFileService) -> ApiResult<()> {
        let opts = if request.uris.len() == 1 {
            RequestOptions::new()
                .with_purchase_ticket()
                .skip_batch_error()
        } else {
            RequestOptions::new().with_purchase_ticket()
        };
        
        self.delete_with_body("/file", request, opts).await
    }
    
    async fn rename_file(&self, request: &RenameFileService) -> ApiResult<FileResponse> {
        self.post(
            "/file/rename",
            request,
            RequestOptions::new().with_purchase_ticket(),
        ).await
    }
    
    async fn move_files(&self, request: &MoveFileService) -> ApiResult<()> {
        let opts = if request.uris.len() == 1 {
            RequestOptions::new()
                .with_purchase_ticket()
                .skip_batch_error()
        } else {
            RequestOptions::new().with_purchase_ticket()
        };
        
        self.post("/file/move", request, opts).await
    }
    
    async fn restore_files(&self, request: &DeleteFileService) -> ApiResult<()> {
        let opts = if request.uris.len() == 1 {
            RequestOptions::new()
                .with_purchase_ticket()
                .skip_batch_error()
        } else {
            RequestOptions::new().with_purchase_ticket()
        };
        
        self.post("/file/restore", request, opts).await
    }
    
    async fn patch_metadata(&self, request: &PatchMetadataService) -> ApiResult<()> {
        let opts = if request.uris.len() == 1 {
            RequestOptions::new()
                .with_purchase_ticket()
                .skip_batch_error()
        } else {
            RequestOptions::new().with_purchase_ticket()
        };
        
        self.patch("/file/metadata", request, opts).await
    }
    
    async fn get_file_url(&self, request: &FileURLService) -> ApiResult<FileURLResponse> {
        let opts = if request.uris.len() == 1 {
            RequestOptions::new()
                .with_purchase_ticket()
                .skip_batch_error()
        } else {
            RequestOptions::new().with_purchase_ticket()
        };
        
        self.post("/file/url", request, opts).await
    }
    
    async fn unlock_files(&self, request: &UnlockFileService) -> ApiResult<()> {
        self.delete_with_body(
            "/file/lock",
            request,
            RequestOptions::new().skip_lock_conflict(),
        ).await
    }
    
    async fn set_current_version(&self, request: &VersionControlService) -> ApiResult<()> {
        self.post(
            "/file/version/current",
            request,
            RequestOptions::new().with_purchase_ticket(),
        ).await
    }
    
    async fn delete_version(&self, request: &VersionControlService) -> ApiResult<()> {
        self.delete_with_body(
            "/file/version",
            request,
            RequestOptions::new().with_purchase_ticket(),
        ).await
    }
    
    async fn update_file(&self, params: &FileUpdateService, data: Bytes) -> ApiResult<FileResponse> {
        // Build query string
        let mut query_params = vec![format!("uri={}", urlencoding::encode(&params.uri))];
        
        if let Some(previous) = &params.previous {
            query_params.push(format!("previous={}", previous));
        }
        
        let query = format!("?{}", query_params.join("&"));
        
        // We need to use a custom request here since we're sending binary data
        let url = self.build_url(&format!("/file/content{}", query));
        let token = self.get_access_token().await?;
        
        let response = self.http_client
            .put(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/octet-stream")
            .body(data)
            .send()
            .await?;
        
        let api_response: crate::error::ApiResponse<FileResponse> = response.json().await?;
        
        if api_response.code != 0 {
            return Err(crate::error::ApiError::from_response(api_response));
        }
        
        api_response.data.ok_or_else(|| {
            crate::error::ApiError::Other("API returned success but no data".to_string())
        })
    }
    
    async fn get_storage_policy_options(&self) -> ApiResult<Vec<StoragePolicy>> {
        self.get("/user/setting/policies", RequestOptions::new()).await
    }
    
    async fn mount_storage_policy(&self, request: &MountPolicyService) -> ApiResult<Vec<StoragePolicy>> {
        self.patch(
            "/file/policy",
            request,
            RequestOptions::new().with_purchase_ticket(),
        ).await
    }
    
    async fn set_permissions(&self, request: &SetPermissionService) -> ApiResult<()> {
        let opts = if request.uris.len() == 1 {
            RequestOptions::new()
                .with_purchase_ticket()
                .skip_batch_error()
        } else {
            RequestOptions::new().with_purchase_ticket()
        };
        
        self.post("/file/permission", request, opts).await
    }
    
    async fn create_upload_session(&self, request: &UploadSessionRequest) -> ApiResult<UploadCredential> {
        self.post(
            "/file/upload",
            request,
            RequestOptions::new().with_purchase_ticket(),
        ).await
    }
    
    async fn upload_chunk(
        &self,
        session_id: &str,
        chunk_index: usize,
        data: Bytes,
    ) -> ApiResult<UploadCredential> {
        let url = self.build_url(&format!("/file/upload/{}/{}", session_id, chunk_index));
        let token = self.get_access_token().await?;
        
        let response = self.http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/octet-stream")
            .body(data)
            .send()
            .await?;
        
        let api_response: crate::error::ApiResponse<UploadCredential> = response.json().await?;
        
        if api_response.code != 0 {
            return Err(crate::error::ApiError::from_response(api_response));
        }
        
        api_response.data.ok_or_else(|| {
            crate::error::ApiError::Other("API returned success but no data".to_string())
        })
    }
    
    async fn delete_upload_session(&self, request: &DeleteUploadSessionService) -> ApiResult<()> {
        self.delete_with_body(
            "/file/upload",
            request,
            RequestOptions::new().with_purchase_ticket(),
        ).await
    }
    
    async fn complete_s3_upload(
        &self,
        policy_type: &str,
        session_id: &str,
        session_key: &str,
    ) -> ApiResult<()> {
        self.get(
            &format!("/callback/{}/{}/{}", policy_type, session_id, session_key),
            RequestOptions::new(),
        ).await
    }
    
    async fn complete_onedrive_upload(&self, session_id: &str, session_key: &str) -> ApiResult<()> {
        self.post::<(), ()>(
            &format!("/callback/onedrive/{}/{}", session_id, session_key),
            &(),
            RequestOptions::new(),
        ).await
    }
}

