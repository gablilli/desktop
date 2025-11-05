use cloudreve_api::api::{ExplorerApi, UserApi};
use cloudreve_api::{Client, ClientConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create client configuration
    let config = ClientConfig::new("https://your-cloudreve-instance.com")
        .with_timeout(30);
    
    let client = Client::new(config);
    
    // Login
    println!("Logging in...");
    let login_response = client.login("user@example.com", "password").await?;
    println!("Logged in as: {}", login_response.user.nickname);
    
    // Set tokens for subsequent requests
    client.set_tokens_with_expiry(&login_response.token).await;
    
    // Get user information
    println!("\nFetching user info...");
    let user = client.get_user_me().await?;
    println!("User ID: {}", user.id);
    println!("Email: {}", user.email.as_ref().unwrap_or(&"N/A".to_string()));
    
    // Get capacity
    println!("\nFetching storage capacity...");
    let capacity = client.get_user_capacity().await?;
    println!(
        "Storage: {} / {} bytes ({:.2}% used)",
        capacity.used,
        capacity.total,
        (capacity.used as f64 / capacity.total as f64) * 100.0
    );
    
    // List files in root directory
    println!("\nListing files in root directory...");
    let params = cloudreve_api::models::explorer::ListFileService {
        uri: "/".to_string(),
        page: None,
        page_size: Some(20),
        order_by: None,
        order_direction: None,
        next_page_token: None,
    };
    
    let files = client.list_files(&params).await?;
    println!("Found {} files/folders:", files.files.len());
    
    for file in &files.files {
        let file_type = if file.file_type == 0 { "File" } else { "Folder" };
        println!(
            "  [{}] {} - {} bytes",
            file_type, file.name, file.size
        );
    }
    
    // Create a test folder
    println!("\nCreating test folder...");
    let create_req = cloudreve_api::models::explorer::CreateFileService {
        uri: "/TestFolder".to_string(),
        file_type: "folder".to_string(),
        err_on_conflict: Some(false),
        metadata: None,
    };
    
    match client.create_file(&create_req).await {
        Ok(folder) => {
            println!("Created folder: {} (ID: {})", folder.name, folder.id);
            
            // Delete the test folder
            println!("\nDeleting test folder...");
            let delete_req = cloudreve_api::models::explorer::DeleteFileService {
                uris: vec!["/TestFolder".to_string()],
                unlink: None,
                skip_soft_delete: None,
            };
            client.delete_files(&delete_req).await?;
            println!("Deleted folder successfully");
        }
        Err(e) => {
            println!("Failed to create folder: {}", e);
        }
    }
    
    println!("\nDone!");
    
    Ok(())
}

