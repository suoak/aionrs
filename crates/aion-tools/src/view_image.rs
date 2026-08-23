use std::path::Path;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use image::{GenericImageView, ImageFormat};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use aion_protocol::events::ToolCategory;
use aion_types::message::{ContentBlock, ImageUrl, extension_to_image_media_type};
use aion_types::tool::{JsonSchema, ToolResult};

use crate::{Tool, ToolExecutionOutput};

const MAX_IMAGE_SIZE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_IMAGE_PIXELS: u64 = 40_000_000;
const MAX_IMAGE_EDGE: u32 = 2048;

struct LoadedImage {
    image_url: ImageUrl,
    metadata: Value,
}

fn detect_image_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

pub struct ViewImageTool;

impl ViewImageTool {
    pub fn new() -> Self {
        Self
    }

    async fn load_image(&self, input: &Value) -> Result<LoadedImage, String> {
        let file_path = input
            .get("file_path")
            .and_then(Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .ok_or_else(|| "Missing required parameter: file_path".to_owned())?;
        let path = Path::new(file_path);
        if !path.is_absolute() {
            return Err("file_path must be an absolute path".to_owned());
        }

        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .ok_or_else(|| "Image path must have a supported extension".to_owned())?;
        let mime_type = extension_to_image_media_type(extension)
            .ok_or_else(|| format!("Unsupported image extension: {extension}"))?;

        let metadata = tokio::fs::metadata(path)
            .await
            .map_err(|error| format!("Failed to read image metadata: {error}"))?;
        if !metadata.is_file() {
            return Err("Image path is not a regular file".to_owned());
        }
        if metadata.len() > MAX_IMAGE_SIZE_BYTES {
            return Err(format!("Image exceeds the {} byte size limit", MAX_IMAGE_SIZE_BYTES));
        }

        let bytes = tokio::fs::read(path)
            .await
            .map_err(|error| format!("Failed to read image: {error}"))?;
        if bytes.len() as u64 > MAX_IMAGE_SIZE_BYTES {
            return Err(format!("Image exceeds the {} byte size limit", MAX_IMAGE_SIZE_BYTES));
        }
        let detected_mime_type = detect_image_media_type(&bytes)
            .ok_or_else(|| "File content is not a supported JPEG, PNG, GIF, or WebP image".to_owned())?;
        if detected_mime_type != mime_type {
            return Err(format!(
                "Image content type {detected_mime_type} does not match extension type {mime_type}"
            ));
        }

        let format = ImageFormat::from_mime_type(detected_mime_type)
            .ok_or_else(|| "Image decoder is unavailable for the detected content type".to_owned())?;
        let (original_width, original_height) = image::ImageReader::with_format(std::io::Cursor::new(&bytes), format)
            .into_dimensions()
            .map_err(|error| format!("Failed to read image dimensions: {error}"))?;
        let pixels = u64::from(original_width).saturating_mul(u64::from(original_height));
        if pixels > MAX_IMAGE_PIXELS {
            return Err(format!("Image exceeds the {MAX_IMAGE_PIXELS} pixel decode limit"));
        }
        let decoded = image::load_from_memory_with_format(&bytes, format)
            .map_err(|error| format!("Failed to decode image: {error}"))?;
        let should_resize = original_width > MAX_IMAGE_EDGE || original_height > MAX_IMAGE_EDGE;
        let (delivered_bytes, delivered_width, delivered_height) = if should_resize {
            let resized = decoded.thumbnail(MAX_IMAGE_EDGE, MAX_IMAGE_EDGE);
            let dimensions = resized.dimensions();
            let mut cursor = std::io::Cursor::new(Vec::new());
            resized
                .write_to(&mut cursor, format)
                .map_err(|error| format!("Failed to encode downscaled image: {error}"))?;
            (cursor.into_inner(), dimensions.0, dimensions.1)
        } else {
            (bytes.clone(), original_width, original_height)
        };
        let scale_x = f64::from(original_width) / f64::from(delivered_width);
        let scale_y = f64::from(original_height) / f64::from(delivered_height);
        let original_sha256 = format!("{:x}", Sha256::digest(&bytes));
        let delivered_sha256 = format!("{:x}", Sha256::digest(&delivered_bytes));
        let image_url = ImageUrl {
            url: format!("data:{detected_mime_type};base64,{}", STANDARD.encode(delivered_bytes)),
        };
        image_url
            .validate()
            .map_err(|error| format!("Failed to prepare image input: {error}"))?;
        Ok(LoadedImage {
            image_url,
            metadata: json!({
                "mime_type": detected_mime_type,
                "original_sha256": original_sha256,
                "delivered_sha256": delivered_sha256,
                "original_dimensions": { "width": original_width, "height": original_height },
                "delivered_dimensions": { "width": delivered_width, "height": delivered_height },
                "coordinate_scale": { "x": scale_x, "y": scale_y },
                "downscaled": should_resize,
            }),
        })
    }

    fn success_result(file_path: &str) -> ToolResult {
        ToolResult {
            content: format!("Image loaded from {file_path} and attached to the next model turn."),
            is_error: false,
        }
    }

    fn error_result(error: String) -> ToolResult {
        ToolResult {
            content: error,
            is_error: true,
        }
    }
}

impl Default for ViewImageTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ViewImageTool {
    fn name(&self) -> &str {
        "ViewImage"
    }

    fn description(&self) -> &str {
        "Loads an image from an absolute local file path and attaches it to the next model turn. Use this when you need to inspect an image attachment."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Absolute path to a JPEG, PNG, GIF, or WebP image"
                }
            },
            "required": ["file_path"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let file_path = input.get("file_path").and_then(Value::as_str).unwrap_or("unknown");
        match self.load_image(&input).await {
            Ok(_) => Self::success_result(file_path),
            Err(error) => Self::error_result(error),
        }
    }

    async fn execute_with_follow_up(&self, input: Value) -> ToolExecutionOutput {
        let file_path = input.get("file_path").and_then(Value::as_str).unwrap_or("unknown");
        match self.load_image(&input).await {
            Ok(loaded) => ToolExecutionOutput {
                result: Self::success_result(file_path),
                follow_up_blocks: vec![ContentBlock::Image {
                    image_url: loaded.image_url,
                }],
                content_blocks: None,
                structured_content: Some(loaded.metadata),
                error_code: None,
                truncation: None,
            },
            Err(error) => Self::error_result(error).into(),
        }
    }

    fn requires_image_input(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let path = input.get("file_path").and_then(Value::as_str).unwrap_or("unknown");
        format!("View image {path}")
    }
}

#[cfg(test)]
#[path = "view_image_test.rs"]
mod view_image_test;
