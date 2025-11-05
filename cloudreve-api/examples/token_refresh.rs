use cloudreve_api::api::{ExplorerApi, UserApi};
use cloudreve_api::{Client, ClientConfig};
use std::time::Duration;

/// This example demonstrates automatic token refresh
/// 
/// The client automatically handles:
/// 1. Detecting when an access token has expired
/// 2. Using the refresh token to get a new access token
/// 3. Retrying the original request with the new token
/// 
/// All of this happens transparently - your code doesn't need to worry about it!
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create client
    let config = ClientConfig::new("https://your-cloudreve-instance.com")
        .with_timeout(30);
    
    let client = Client::new(config);
    
    // Login
    println!("=== Logging in ===");
    let login_response = client.login("user@example.com", "password").await?;
    println!("✓ Logged in as: {}", login_response.user.nickname);
    
    // Set tokens - this stores both access and refresh tokens
    client.set_tokens_with_expiry(&login_response.token).await;
    println!("✓ Tokens stored");
    
    // Make some API calls - the client will use the access token
    println!("\n=== Making API calls with fresh token ===");
    
    let user = client.get_user_me().await?;
    println!("✓ Got user info: {}", user.id);
    
    let capacity = client.get_user_capacity().await?;
    println!("✓ Got capacity: {} / {} bytes", capacity.used, capacity.total);
    
    // Simulate token expiration by manually setting expired tokens
    // In a real scenario, this would happen naturally after ~1 hour
    println!("\n=== Simulating token expiration ===");
    println!("(In a real application, this would happen after the access token expires)");
    
    // Let's manually trigger a refresh by clearing the access token
    // but keeping the refresh token (simulating expiration)
    use chrono::Utc;
    let expired_token = cloudreve_api::models::user::Token {
        access_token: "expired_token".to_string(),
        refresh_token: login_response.token.refresh_token.clone(),
        access_expires: (Utc::now() - chrono::Duration::hours(1))
            .to_rfc3339(),
        refresh_expires: (Utc::now() + chrono::Duration::days(7))
            .to_rfc3339(),
    };
    client.set_tokens_with_expiry(&expired_token).await;
    
    println!("✓ Access token marked as expired");
    
    // Now make another API call
    // The client will detect the expired token, refresh it, and retry
    println!("\n=== Making API call with expired token ===");
    println!("The client will automatically:");
    println!("  1. Detect that the access token is expired");
    println!("  2. Use the refresh token to get a new access token");
    println!("  3. Retry the original request with the new token");
    println!();
    
    match client.get_user_me().await {
        Ok(user) => {
            println!("✓ Request succeeded! User: {}", user.id);
            println!("  (Token was automatically refreshed)");
        }
        Err(e) => {
            // This might fail if we're using fake tokens
            println!("✗ Request failed: {}", e);
            println!("  (This is expected with the simulated expiration)");
        }
    }
    
    // Example: Handling errors
    println!("\n=== Error Handling ===");
    
    // Clear all tokens to demonstrate login required error
    client.clear_tokens().await;
    
    match client.get_user_me().await {
        Ok(_) => println!("Unexpected success"),
        Err(e) => {
            println!("✓ Got expected error: {}", e);
            
            // Check error type
            if e.requires_login() {
                println!("  → This error requires re-authentication");
            }
        }
    }
    
    println!("\n=== Summary ===");
    println!("Key features demonstrated:");
    println!("  ✓ Automatic token refresh on expiration");
    println!("  ✓ Transparent retry of failed requests");
    println!("  ✓ Proper error handling for auth failures");
    println!("  ✓ No manual token management required");
    
    Ok(())
}

