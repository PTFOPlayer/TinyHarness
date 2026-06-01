use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Maximum image file size: 20 MB.
/// Larger files are rejected to avoid memory issues.
pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// Supported image MIME types for multimodal models.
pub const SUPPORTED_MIME_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/jpg",
    "image/webp",
    "image/gif",
    "image/bmp",
];

/// An image attachment for multimodal chat messages.
///
/// Stores both the filesystem path (for reference) and the base64-encoded
/// content (for self-contained session persistence).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAttachment {
    /// Original path the image was loaded from (for display and `/image` listing).
    pub path: PathBuf,
    /// MIME type (e.g. "image/png", "image/jpeg").
    pub mime_type: String,
    /// Base64-encoded image bytes (without the `data:` prefix).
    pub base64_data: String,
    /// File size in bytes.
    pub size_bytes: u64,
    /// Image dimensions: (width, height) in pixels, if detectable.
    /// Currently not parsed; reserved for future use.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub dimensions: Option<(u32, u32)>,
}

/// Error type for image loading operations.
#[derive(Debug)]
pub enum ImageError {
    FileNotFound(PathBuf),
    IoError(std::io::Error),
    TooLarge { path: PathBuf, size: u64, max: u64 },
    UnsupportedMime { path: PathBuf, mime: String },
    EncodeError(String),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageError::FileNotFound(p) => write!(f, "File not found: {}", p.display()),
            ImageError::IoError(e) => write!(f, "I/O error: {}", e),
            ImageError::TooLarge { path, size, max } => {
                write!(
                    f,
                    "Image too large: {} ({} bytes, max {} bytes)",
                    path.display(),
                    size,
                    max
                )
            }
            ImageError::UnsupportedMime { path, mime } => {
                write!(
                    f,
                    "Unsupported image format '{}' for {}",
                    mime,
                    path.display()
                )
            }
            ImageError::EncodeError(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for ImageError {}

/// Guess MIME type from a file extension.
fn guess_mime(ext: &str) -> Option<&'static str> {
    match ext.to_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "bmp" => Some("image/bmp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// Base64 character set (standard, with '+' and '/').
const BASE64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode arbitrary bytes as a standard base64 string.
fn encode_base64(bytes: &[u8]) -> String {
    let mut result = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);

        let i0 = (b0 >> 2) as usize;
        let i1 = (((b0 & 0x03) << 4) | (b1 >> 4)) as usize;
        let i2 = if chunk.len() > 1 {
            (((b1 & 0x0f) << 2) | (b2 >> 6)) as usize
        } else {
            64
        };
        let i3 = if chunk.len() > 2 {
            (b2 & 0x3f) as usize
        } else {
            64
        };

        result.push(BASE64_CHARS[i0] as char);
        result.push(BASE64_CHARS[i1] as char);
        result.push(if i2 < 64 { BASE64_CHARS[i2] } else { b'=' } as char);
        result.push(if i3 < 64 { BASE64_CHARS[i3] } else { b'=' } as char);
    }

    result
}

impl ImageAttachment {
    /// Load an image from disk, encode it as base64, and validate.
    ///
    /// Rejects files larger than `MAX_IMAGE_BYTES` and files with unsupported
    /// MIME types (based on extension).
    pub fn load(path: PathBuf) -> Result<Self, ImageError> {
        // Read file bytes
        let bytes = std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ImageError::FileNotFound(path.clone())
            } else {
                ImageError::IoError(e)
            }
        })?;

        let size = bytes.len() as u64;

        // Size check
        if size > MAX_IMAGE_BYTES {
            return Err(ImageError::TooLarge {
                path: path.clone(),
                size,
                max: MAX_IMAGE_BYTES,
            });
        }

        // Guess MIME type from extension
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let mime = guess_mime(ext).ok_or_else(|| ImageError::UnsupportedMime {
            path: path.clone(),
            mime: ext.to_string(),
        })?;

        // Base64 encode (pure std, no extra deps)
        let base64_data = encode_base64(&bytes);

        Ok(ImageAttachment {
            path,
            mime_type: mime.to_string(),
            base64_data,
            size_bytes: size,
            dimensions: None,
        })
    }

    /// Load an image from disk with an absolute path.
    /// Convenience wrapper that canonicalises the path.
    pub fn load_from_str(path_str: &str) -> Result<Self, ImageError> {
        let path = PathBuf::from_str(path_str)
            .map_err(|_| ImageError::FileNotFound(PathBuf::from(path_str)))?;
        let abs_path = if path.is_absolute() {
            path
        } else {
            // Resolve relative to CWD
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(&path)
        };
        Self::load(abs_path)
    }

    /// Format as a `data:` URI for use in OpenAI-compatible APIs.
    pub fn data_uri(&self) -> String {
        format!("data:{};base64,{}", self.mime_type, self.base64_data)
    }

    /// Display name (file name, fallback to full path).
    pub fn display_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.to_string_lossy().to_string())
    }

    /// Human-readable size.
    pub fn size_display(&self) -> String {
        let bytes = self.size_bytes;
        if bytes < 1024 {
            format!("{} B", bytes)
        } else if bytes < 1024 * 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else {
            format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
        }
    }

    /// Returns true if the file still exists on disk.
    pub fn exists_on_disk(&self) -> bool {
        self.path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_base64() {
        assert_eq!(encode_base64(b"f"), "Zg==");
        assert_eq!(encode_base64(b"fo"), "Zm8=");
        assert_eq!(encode_base64(b"foo"), "Zm9v");
        assert_eq!(encode_base64(b"foob"), "Zm9vYg==");
        assert_eq!(encode_base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode_base64(b"foobar"), "Zm9vYmFy");
        // Empty input
        assert_eq!(encode_base64(b""), "");
    }

    use std::io::Write;

    fn make_png() -> Vec<u8> {
        // Minimal valid PNG (1x1 black pixel)
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, // PNG signature
            0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, // IHDR chunk
            0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
            0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, // IDAT chunk
            0x54, 0x08, 0xD7, 0x63, 0xF8, 0xFF, 0xFF, 0x3F, 0x00, 0x05, 0xFE, 0x02, 0xFE, 0xDC,
            0xCC, 0x59, 0xE7, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, // IEND chunk
            0x44, 0xAE, 0x42, 0x60, 0x82,
        ]
    }

    #[test]
    fn test_load_png() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.png");
        let png_bytes = make_png();
        let mut f = std::fs::File::create(&file_path).unwrap();
        f.write_all(&png_bytes).unwrap();

        let img = ImageAttachment::load(file_path.clone()).unwrap();
        assert_eq!(img.mime_type, "image/png");
        assert!(!img.base64_data.is_empty());
        assert_eq!(img.size_bytes, png_bytes.len() as u64);
        assert!(img.data_uri().starts_with("data:image/png;base64,"));
    }

    #[test]
    fn test_load_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("photo.jpg");
        // Minimal JPEG (not valid, but extension test should work)
        std::fs::write(&file_path, b"\xff\xd8\xff\xe0").unwrap();

        let img = ImageAttachment::load(file_path).unwrap();
        assert_eq!(img.mime_type, "image/jpeg");
    }

    #[test]
    fn test_load_unsupported_extension() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("doc.pdf");
        std::fs::write(&file_path, b"not a pdf").unwrap();

        let err = ImageAttachment::load(file_path).unwrap_err();
        match err {
            ImageError::UnsupportedMime { .. } => {}
            other => panic!("expected UnsupportedMime, got {:?}", other),
        }
    }

    #[test]
    fn test_load_nonexistent() {
        let path = PathBuf::from("/nonexistent/image.png");
        let err = ImageAttachment::load(path).unwrap_err();
        match err {
            ImageError::FileNotFound(_) => {}
            other => panic!("expected FileNotFound, got {:?}", other),
        }
    }

    #[test]
    fn test_display_name() {
        let img = ImageAttachment {
            path: PathBuf::from("/tmp/screenshot.png"),
            mime_type: "image/png".to_string(),
            base64_data: "aaaa".to_string(),
            size_bytes: 100,
            dimensions: None,
        };
        assert_eq!(img.display_name(), "screenshot.png");
        assert_eq!(img.size_display(), "100 B");
    }

    #[test]
    fn test_size_display() {
        let img = ImageAttachment {
            path: PathBuf::from("/tmp/big.png"),
            mime_type: "image/png".to_string(),
            base64_data: "x".repeat(1024 * 1024),
            size_bytes: 1_500_000,
            dimensions: None,
        };
        assert!(img.size_display().contains("1.4 MB"));
    }
}
