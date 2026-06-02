use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

use crate::extract_args;
use crate::image::SUPPORTED_MIME_TYPES;
use crate::tools::tool::{BoxFuture, ToolCategory, build_string_params_schema, make_tool};

pub fn read_tool_entry() -> crate::tools::tool::Tool {
    make_tool(
        "read",
        "Read file content. Returns the entire file or a specific line range if from/to are provided. For image files (png, jpg, webp, gif, bmp), returns a description and the image data is automatically loaded for the model to view.",
        ToolCategory::ReadOnly,
        build_string_params_schema(
            &[("path", "The absolute path to the file to read")],
            &[
                ("from", "Starting line number (0-based, inclusive)", "0"),
                (
                    "to",
                    "Number of lines to read (if from is set, reads this many lines)",
                    "",
                ),
            ],
        ),
        |args| Box::pin(read_tool(args)),
    )
}

pub fn read_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        extract_args!(args, path);

        // Check if this is an image file — skip binary read, return description only.
        // The agent layer will load the image as an ImageAttachment separately.
        if is_image_file(&path) {
            let dims = detect_image_dimensions(&path);
            let dims_str = dims
                .map(|(w, h)| format!("{}x{}px, ", w, h))
                .unwrap_or_default();
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            let size_str = format_image_size(size);
            let mime = guess_image_mime(&path).unwrap_or("unknown");
            return format!(
                "[IMAGE] {0}\nImage file: '{0}' ({1}{2} {3})\n\
                This is an image — its content has been automatically loaded so you can view it.",
                path, dims_str, size_str, mime,
            );
        }

        // Check if partial reading is requested
        let from = args.get("from").and_then(|f| f.parse::<usize>().ok());
        let to = args.get("to").and_then(|t| t.parse::<usize>().ok());

        match (from, to) {
            (Some(from), Some(to)) => read_partial(&path, from, to),
            _ => match fs::read_to_string(&path) {
                Ok(content) => {
                    let line_count = content.lines().count();
                    format!("Read '{}' ({} lines)\n{}", path, line_count, content)
                }
                Err(e) => format!("Error reading file: {}", e),
            },
        }
    })
}

/// Check if a path refers to an image file by extension.
pub fn is_image_file(path: &str) -> bool {
    let lowercase = path.to_lowercase();
    SUPPORTED_MIME_TYPES.iter().any(|mime| {
        let ext = mime.strip_prefix("image/").unwrap_or(mime);
        lowercase.ends_with(&format!(".{}", ext)) || (ext == "jpeg" && lowercase.ends_with(".jpg"))
    })
}

/// Guess the MIME type of an image file from its extension.
pub fn guess_image_mime(path: &str) -> Option<&'static str> {
    let lowercase = path.to_lowercase();
    for &mime in SUPPORTED_MIME_TYPES {
        let ext = mime.strip_prefix("image/").unwrap_or(mime);
        if lowercase.ends_with(&format!(".{}", ext))
            || (ext == "jpeg" && lowercase.ends_with(".jpg"))
        {
            return Some(mime);
        }
    }
    None
}

/// Parse image dimensions from a PNG or JPEG file header.
/// Returns `None` for unsupported formats or on error.
pub fn detect_image_dimensions(path: &str) -> Option<(u32, u32)> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 24 {
        return None;
    }

    // PNG: width at offset 16, height at offset 20 (after signature + IHDR)
    if data[0..8] == [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A] {
        let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return Some((w, h));
    }

    // JPEG: look for SOF0/1/2 marker (0xFF 0xC0/0xC1/0xC2)
    if data[0..2] == [0xFF, 0xD8] {
        let mut i = 2;
        while i + 10 < data.len() {
            if data[i] != 0xFF {
                break;
            }
            let marker = data[i + 1];
            if marker == 0xC0 || marker == 0xC1 || marker == 0xC2 {
                let h = u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32;
                let w = u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32;
                return Some((w, h));
            }
            // Skip to next marker segment
            let seg_len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
            i += 2 + seg_len;
        }
    }

    None
}

/// Format a byte size for display.
fn format_image_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn read_partial(path: &str, from: usize, to: usize) -> String {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(err) => return format!("Error: {}", err),
    };

    let reader = BufReader::new(file);

    let mut content = String::new();
    let mut lines_read = 0usize;

    for line in reader.lines().skip(from).take(to).flatten() {
        content.push_str(&line);
        content.push('\n');
        lines_read += 1;
    }

    if content.is_empty() {
        format!("Error: No lines to read in '{}' at offset {}", path, from)
    } else {
        format!(
            "Read '{}' ({} lines, starting at line {})\n{}",
            path, lines_read, from, content
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_is_image_file_png() {
        assert!(is_image_file("screenshot.png"));
        assert!(is_image_file("/tmp/photo.PNG"));
    }

    #[test]
    fn test_is_image_file_jpg() {
        assert!(is_image_file("photo.jpg"));
        assert!(is_image_file("photo.jpeg"));
        assert!(is_image_file("photo.JPG"));
    }

    #[test]
    fn test_is_image_file_not_image() {
        assert!(!is_image_file("main.rs"));
        assert!(!is_image_file("Cargo.toml"));
        assert!(!is_image_file("data.jsonl"));
    }

    #[test]
    fn test_detect_png_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.png");
        // Minimal valid PNG: 2x3 pixels (width=2, height=3)
        let png = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR
            0x00, 0x00, 0x00, 0x02, // width  = 2
            0x00, 0x00, 0x00, 0x03, // height = 3
            0x08, 0x02, 0x00, 0x00, 0x00, // bit depth + color type + etc
            0x90, 0x77, 0x53, 0xDE, // IHDR CRC
            0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, // IEND
            0xAE, 0x42, 0x60, 0x82,
        ];
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&png).unwrap();

        let dims = detect_image_dimensions(&path.to_string_lossy());
        assert_eq!(dims, Some((2, 3)));
    }

    #[test]
    fn test_guess_image_mime() {
        assert_eq!(guess_image_mime("photo.png"), Some("image/png"));
        assert_eq!(guess_image_mime("photo.jpg"), Some("image/jpeg"));
        assert_eq!(guess_image_mime("photo.jpeg"), Some("image/jpeg"));
        assert_eq!(guess_image_mime("main.rs"), None);
    }
}
