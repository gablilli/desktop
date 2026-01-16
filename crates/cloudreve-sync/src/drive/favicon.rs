use anyhow::{Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Manifest.json structure
#[derive(Debug, Deserialize)]
struct ManifestIcon {
    sizes: String,
    src: String,
    #[serde(rename = "type")]
    icon_type: String,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    icons: Vec<ManifestIcon>,
}

/// Get the icons directory path
fn get_icons_dir() -> Result<PathBuf> {
    let home_dir = dirs::home_dir().context("Failed to get user home directory")?;
    let icons_dir = home_dir.join(".cloudreve").join("icos");

    // Ensure icons directory exists
    if !icons_dir.exists() {
        std::fs::create_dir_all(&icons_dir).context("Failed to create icons directory")?;
    }

    Ok(icons_dir)
}

/// Parse icon size from sizes string (e.g., "192x192" or "64x64 32x32")
fn parse_icon_size(sizes: &str) -> Option<u32> {
    sizes
        .split_whitespace()
        .filter_map(|size| size.split('x').next().and_then(|s| s.parse::<u32>().ok()))
        .min()
}

/// Fetch and save favicon from instance_url
pub async fn fetch_and_save_favicon(instance_url: &str) -> Result<String> {
    tracing::info!(target: "drive::favicon", instance_url = %instance_url, "Fetching favicon");

    // Parse the URL to get hostname and port
    let parsed_url = url::Url::parse(instance_url).context("Failed to parse instance URL")?;

    let host_with_port = if let Some(port) = parsed_url.port() {
        format!("{}:{}", parsed_url.host_str().unwrap_or(""), port)
    } else {
        parsed_url.host_str().unwrap_or("").to_string()
    };

    // Generate SHA256 hash of hostname:port
    let mut hasher = Sha256::new();
    hasher.update(host_with_port.as_bytes());
    let hash_hex = format!("{:x}", hasher.finalize());
    let hash = &hash_hex[..16];

    // Get icons directory
    let icons_dir = get_icons_dir()?;
    let icon_path = icons_dir.join(format!("{}.ico", hash));

    // Check if icon already exists
    if icon_path.exists() {
        tracing::debug!(target: "drive::favicon", path = %icon_path.display(), "Icon already exists");
        return Ok(icon_path.to_string_lossy().to_string());
    }

    // Fetch manifest.json
    let manifest_url = format!("{}/manifest.json", instance_url.trim_end_matches('/'));
    tracing::debug!(target: "drive::favicon", manifest_url = %manifest_url, "Fetching manifest.json");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("Failed to create HTTP client")?;

    let manifest: Manifest = client
        .get(&manifest_url)
        .send()
        .await
        .context("Failed to fetch manifest.json")?
        .json()
        .await
        .context("Failed to parse manifest.json")?;

    // Find the smallest icon
    let smallest_icon = manifest
        .icons
        .iter()
        .filter_map(|icon| parse_icon_size(&icon.sizes).map(|size| (size, icon)))
        .min_by_key(|(size, _)| *size)
        .map(|(_, icon)| icon)
        .context("No valid icons found in manifest")?;

    tracing::debug!(target: "drive::favicon", icon_src = %smallest_icon.src, sizes = %smallest_icon.sizes, "Selected icon");

    // Download the icon
    let icon_url = if smallest_icon.src.starts_with("http") {
        smallest_icon.src.clone()
    } else {
        let base = instance_url.trim_end_matches('/');
        let path = smallest_icon.src.trim_start_matches('/');
        if smallest_icon.src.starts_with('/') {
            format!("{}{}", base, smallest_icon.src)
        } else {
            format!("{}/{}", base, path)
        }
    };

    tracing::debug!(target: "drive::favicon", icon_url = %icon_url, "Downloading icon");

    let icon_bytes = client
        .get(&icon_url)
        .send()
        .await
        .context("Failed to download icon")?
        .bytes()
        .await
        .context("Failed to read icon bytes")?;

    // Convert to ICO format if needed
    if smallest_icon.icon_type.contains("x-icon") || icon_url.ends_with(".ico") {
        // Already an ICO file, save directly
        std::fs::write(&icon_path, &icon_bytes).context("Failed to save icon file")?;
    } else {
        // Convert image to ICO format
        let img = image::load_from_memory(&icon_bytes).context("Failed to load image")?;

        // Resize to 32x32 for ICO (standard size)
        let resized = img.resize(64, 64, image::imageops::FilterType::Lanczos3);

        // Save as ICO
        resized
            .save_with_format(&icon_path, image::ImageFormat::Ico)
            .context("Failed to save as ICO")?;
    }

    tracing::info!(target: "drive::favicon", path = %icon_path.display(), "Favicon saved successfully");

    Ok(icon_path.to_string_lossy().to_string())
}
