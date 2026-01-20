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

/// Result containing paths to both the ICO icon and raw image
#[derive(Debug, Clone)]
pub struct FaviconResult {
    /// Path to the ICO file (for Windows shell integration)
    pub ico_path: String,
    /// Path to the raw image file (PNG/JPG/etc, before ICO conversion)
    pub raw_path: String,
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
/// Returns the first (typically largest for multi-size) dimension
fn parse_icon_size(sizes: &str) -> Option<u32> {
    sizes
        .split_whitespace()
        .filter_map(|size| size.split('x').next().and_then(|s| s.parse::<u32>().ok()))
        .next()
}

/// Fetch and save favicon from instance_url
/// Returns both the ICO path and the raw image path
/// For ICO: downloads the smallest icon for Windows shell integration
/// For raw: downloads the largest icon for status UI display
pub async fn fetch_and_save_favicon(instance_url: &str) -> Result<FaviconResult> {
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

    // Find the smallest icon for ICO (Windows shell integration)
    let smallest_icon = manifest
        .icons
        .iter()
        .filter_map(|icon| parse_icon_size(&icon.sizes).map(|size| (size, icon)))
        .min_by_key(|(size, _)| *size)
        .map(|(_, icon)| icon)
        .context("No valid icons found in manifest")?;

    // Find the largest icon for raw image (status UI display)
    let largest_icon = manifest
        .icons
        .iter()
        .filter_map(|icon| parse_icon_size(&icon.sizes).map(|size| (size, icon)))
        .max_by_key(|(size, _)| *size)
        .map(|(_, icon)| icon)
        .unwrap_or(smallest_icon); // Fallback to smallest if max fails

    tracing::debug!(target: "drive::favicon", smallest_src = %smallest_icon.src, smallest_sizes = %smallest_icon.sizes, "Selected smallest icon for ICO");
    tracing::debug!(target: "drive::favicon", largest_src = %largest_icon.src, largest_sizes = %largest_icon.sizes, "Selected largest icon for raw");

    // Helper to build full URL from icon src
    let build_icon_url = |icon: &ManifestIcon| -> String {
        if icon.src.starts_with("http") {
            icon.src.clone()
        } else {
            let base = instance_url.trim_end_matches('/');
            let path = icon.src.trim_start_matches('/');
            if icon.src.starts_with('/') {
                format!("{}{}", base, icon.src)
            } else {
                format!("{}/{}", base, path)
            }
        }
    };

    // Download the smallest icon for ICO conversion
    let smallest_icon_url = build_icon_url(smallest_icon);
    tracing::debug!(target: "drive::favicon", icon_url = %smallest_icon_url, "Downloading smallest icon for ICO");

    let smallest_icon_bytes = client
        .get(&smallest_icon_url)
        .send()
        .await
        .context("Failed to download smallest icon")?
        .bytes()
        .await
        .context("Failed to read smallest icon bytes")?;

    // Download the largest icon for raw image (only if different from smallest)
    let (largest_icon_url, largest_icon_bytes) = if largest_icon.src != smallest_icon.src {
        let url = build_icon_url(largest_icon);
        tracing::debug!(target: "drive::favicon", icon_url = %url, "Downloading largest icon for raw");
        let bytes = client
            .get(&url)
            .send()
            .await
            .context("Failed to download largest icon")?
            .bytes()
            .await
            .context("Failed to read largest icon bytes")?;
        (url, bytes)
    } else {
        (smallest_icon_url.clone(), smallest_icon_bytes.clone())
    };

    // Determine raw image extension from largest icon type or URL
    let raw_extension = if largest_icon.icon_type.contains("png") {
        "png"
    } else if largest_icon.icon_type.contains("jpeg") || largest_icon.icon_type.contains("jpg") {
        "jpg"
    } else if largest_icon.icon_type.contains("x-icon") || largest_icon.icon_type.contains("ico") {
        "ico"
    } else if largest_icon_url.ends_with(".png") {
        "png"
    } else if largest_icon_url.ends_with(".jpg") || largest_icon_url.ends_with(".jpeg") {
        "jpg"
    } else if largest_icon_url.ends_with(".ico") {
        "ico"
    } else {
        "png" // Default to PNG
    };

    let raw_path = icons_dir.join(format!("{}_raw.{}", hash, raw_extension));

    // Save the raw image (largest icon)
    std::fs::write(&raw_path, &largest_icon_bytes).context("Failed to save raw icon file")?;
    tracing::debug!(target: "drive::favicon", path = %raw_path.display(), "Raw icon saved");

    // Convert smallest icon to ICO format if needed
    if smallest_icon.icon_type.contains("x-icon") || smallest_icon_url.ends_with(".ico") {
        // Already an ICO file, save directly (also as .ico)
        std::fs::write(&icon_path, &smallest_icon_bytes).context("Failed to save icon file")?;
    } else {
        // Convert image to ICO format
        let img = image::load_from_memory(&smallest_icon_bytes).context("Failed to load image")?;

        // Resize to 64x64 for ICO
        let resized = img.resize(64, 64, image::imageops::FilterType::Lanczos3);

        // Save as ICO
        resized
            .save_with_format(&icon_path, image::ImageFormat::Ico)
            .context("Failed to save as ICO")?;
    }

    tracing::info!(target: "drive::favicon", ico_path = %icon_path.display(), raw_path = %raw_path.display(), "Favicon saved successfully");

    Ok(FaviconResult {
        ico_path: icon_path.to_string_lossy().to_string(),
        raw_path: raw_path.to_string_lossy().to_string(),
    })
}
