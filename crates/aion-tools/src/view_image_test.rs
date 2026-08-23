use std::fs;

use serde_json::json;
use tempfile::TempDir;

use aion_types::message::ContentBlock;

use super::ViewImageTool;
use crate::{Tool, ToolCallContext};

#[tokio::test]
async fn returns_image_as_follow_up_block() {
    let directory = TempDir::new().expect("temp dir");
    let path = directory.path().join("sample.png");
    image::DynamicImage::new_rgba8(1, 1)
        .save_with_format(&path, image::ImageFormat::Png)
        .expect("write image fixture");
    let tool = ViewImageTool::new();

    let output = tool.execute_with_follow_up(json!({ "file_path": path })).await;

    assert!(!output.result.is_error);
    assert_eq!(output.follow_up_blocks.len(), 1);
    assert!(matches!(
        &output.follow_up_blocks[0],
        ContentBlock::Image { image_url }
            if image_url.url.starts_with("data:image/png;base64,")
    ));
    assert_eq!(output.structured_content.as_ref().unwrap()["downscaled"], false);
    assert_eq!(
        output.structured_content.as_ref().unwrap()["original_dimensions"]["width"],
        1
    );
}

#[tokio::test]
async fn downscales_large_images_and_reports_coordinate_scale() {
    let directory = TempDir::new().expect("temp dir");
    let path = directory.path().join("large.png");
    image::DynamicImage::new_rgb8(4096, 2048)
        .save_with_format(&path, image::ImageFormat::Png)
        .expect("write large PNG fixture");
    let tool = ViewImageTool::new();

    let output = tool.execute_with_follow_up(json!({ "file_path": path })).await;

    assert!(!output.result.is_error);
    let metadata = output.structured_content.expect("structured metadata");
    assert_eq!(metadata["delivered_dimensions"]["width"], 2048);
    assert_eq!(metadata["delivered_dimensions"]["height"], 1024);
    assert_eq!(metadata["coordinate_scale"]["x"], 2.0);
    assert_eq!(metadata["coordinate_scale"]["y"], 2.0);
    assert_eq!(metadata["downscaled"], true);
}

#[tokio::test]
async fn rejects_images_over_the_decode_pixel_limit() {
    let directory = TempDir::new().expect("temp dir");
    let path = directory.path().join("too-large.png");
    image::DynamicImage::new_luma8(8000, 5001)
        .save_with_format(&path, image::ImageFormat::Png)
        .expect("write oversized PNG fixture");
    let tool = ViewImageTool::new();

    let output = tool.execute_with_follow_up(json!({ "file_path": path })).await;

    assert!(output.result.is_error);
    assert!(output.result.content.contains("pixel decode limit"));
    assert!(output.follow_up_blocks.is_empty());
}

#[tokio::test]
async fn rejects_file_with_image_extension_but_invalid_content() {
    let directory = TempDir::new().expect("temp dir");
    let path = directory.path().join("sample.png");
    fs::write(&path, b"fake-png").expect("write invalid image fixture");
    let tool = ViewImageTool::new();

    let output = tool.execute_with_follow_up(json!({ "file_path": path })).await;

    assert!(output.result.is_error);
    assert!(output.result.content.contains("File content is not a supported"));
    assert!(output.follow_up_blocks.is_empty());
}

#[tokio::test]
async fn rejects_truncated_image_after_dimension_probe() {
    let directory = TempDir::new().expect("temp dir");
    let path = directory.path().join("truncated.png");
    let truncated_png = [
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8,
        6, 0, 0, 0,
    ];
    fs::write(&path, truncated_png).expect("write truncated image fixture");

    let output = ViewImageTool::new()
        .execute_with_follow_up(json!({ "file_path": path }))
        .await;

    assert!(output.result.is_error);
    assert!(output.result.content.contains("Failed to"));
    assert!(output.follow_up_blocks.is_empty());
}

#[tokio::test]
async fn rejects_image_content_that_does_not_match_extension() {
    let directory = TempDir::new().expect("temp dir");
    let path = directory.path().join("sample.jpg");
    fs::write(&path, b"\x89PNG\r\n\x1a\n").expect("write mismatched image fixture");
    let tool = ViewImageTool::new();

    let output = tool.execute_with_follow_up(json!({ "file_path": path })).await;

    assert!(output.result.is_error);
    assert!(output.result.content.contains("does not match extension type"));
    assert!(output.follow_up_blocks.is_empty());
}

#[tokio::test]
async fn rejects_relative_paths_without_follow_up() {
    let tool = ViewImageTool::new();

    let output = tool
        .execute_with_follow_up(json!({ "file_path": "relative.png" }))
        .await;

    assert!(output.result.is_error);
    assert!(output.result.content.contains("absolute path"));
    assert!(output.follow_up_blocks.is_empty());
}

#[tokio::test]
async fn rejects_unsupported_image_extensions() {
    let directory = TempDir::new().expect("temp dir");
    let path = directory.path().join("sample.svg");
    fs::write(&path, b"<svg/>").expect("write image fixture");
    let tool = ViewImageTool::new();

    let output = tool.execute_with_follow_up(json!({ "file_path": path })).await;

    assert!(output.result.is_error);
    assert!(output.result.content.contains("Unsupported image extension"));
    assert!(output.follow_up_blocks.is_empty());
}

#[tokio::test]
async fn honors_pre_canceled_execution_context() {
    let cancellation = tokio_util::sync::CancellationToken::new();
    cancellation.cancel();
    let context = ToolCallContext {
        execution_id: "view-image-test".to_owned(),
        cancellation,
    };

    let output = ViewImageTool::new()
        .execute_with_context(json!({ "file_path": "C:\\missing.png" }), &context)
        .await;

    assert!(output.result.is_error);
    assert_eq!(
        output.error_code,
        Some(aion_types::tool::ToolExecutionErrorCode::Canceled)
    );
    assert!(output.follow_up_blocks.is_empty());
}
