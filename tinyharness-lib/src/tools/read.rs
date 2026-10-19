use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

use crate::extract_args;
use crate::image::SUPPORTED_MIME_TYPES;
use crate::tools::tool::{BoxFuture, ToolCategory, build_string_params_schema, make_tool};
use serde_json::Value;

/// Maximum characters kept from a single notebook output value (e.g. a huge
/// `text/plain` dataframe dump). Larger values are truncated with a marker.
const MAX_NOTEBOOK_VALUE_CHARS: usize = 4_000;

/// Hard cap for a single line in the notebook fallback scrubber, used when the
/// `.ipynb` file is not valid JSON (or is minified onto one line) and the
/// structure-aware sanitizer cannot run.
const MAX_SCRUBBED_LINE_CHARS: usize = 2_000;

pub fn read_tool_entry() -> crate::tools::tool::Tool {
    make_tool(
        "read",
        "Read file content. Returns the entire file or a specific line range if from/to are provided. For image files (png, jpg, webp, gif, bmp), returns a description and the image data is automatically loaded for the model to view. For Jupyter notebooks (.ipynb), renders the JSON with base64 image outputs stripped and oversized text truncated — pass from/to to page through the rendered result.",
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

        // Jupyter notebooks: embedded base64 image outputs routinely reach
        // hundreds of kilobytes per cell and blow the model's input limit, so
        // render a structure-aware, image-free view of the notebook instead of
        // dumping the raw JSON. `from`/`to` page through the rendered text.
        if is_notebook_file(&path) {
            return read_notebook(&path, from, to);
        }

        match (from, to) {
            (Some(from), Some(to)) => read_partial(&path, from, to),
            _ => match fs::read_to_string(&path) {
                Ok(content) => {
                    let line_count = content.lines().count();
                    format!("({} lines)\n{}", line_count, content)
                }
                Err(e) => format!("Error reading file: {}", e),
            },
        }
    })
}

/// Check if a path refers to a Jupyter notebook by extension.
pub fn is_notebook_file(path: &str) -> bool {
    path.to_lowercase().ends_with(".ipynb")
}

/// Read a Jupyter notebook as compact, model-safe JSON.
///
/// Notebook cells routinely embed rendered figures as base64 payloads
/// (`"image/png": "iVBOR…"` — hundreds of kilobytes *on a single line*), which
/// can push a request past the provider's input-length limit. The JSON is
/// therefore parsed and sanitized before it is returned:
///
/// * every binary payload (image/*, `application/pdf`) is replaced by a
///   short placeholder recording the mime type and the stripped size — this
///   covers both `outputs[].data` and markdown `attachments`;
/// * remaining text values longer than [`MAX_NOTEBOOK_VALUE_CHARS`] are
///   truncated with a marker, so a giant `text/plain` dump cannot recreate
///   the same problem;
/// * cell sources, stream text, `text/plain` results and execution counts are
///   preserved verbatim.
///
/// If the file is not valid JSON (or is a notebook-shaped document the
/// sanitizer cannot walk), the raw text is returned with over-long lines
/// truncated instead, so the base64 blow-up is still neutralised.
///
/// `from`/`to` page through the *rendered* text, not the on-disk file, so the
/// line numbers stay stable across reads.
fn read_notebook(path: &str, from: Option<usize>, to: Option<usize>) -> String {
    let raw = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) => return format!("Error reading file: {e}"),
    };

    let (rendered, mut notes) = match serde_json::from_str::<Value>(&raw) {
        Ok(mut value) => {
            let stripped = strip_binary_payloads(&mut value);
            let truncated = truncate_oversized_strings(&mut value);
            let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| raw.clone());
            let mut notes = Vec::new();
            if stripped > 0 {
                notes.push(format!("{stripped} image outputs stripped"));
            }
            if truncated > 0 {
                notes.push(format!("{truncated} oversized outputs truncated"));
            }
            (text, notes)
        }
        Err(_) => (scrub_huge_lines(&raw), vec!["long lines truncated".into()]),
    };

    let rendered = rendered.trim_end_matches('\n');
    match (from, to) {
        (Some(from), Some(to)) => {
            let selected: Vec<&str> = rendered.lines().skip(from).take(to).collect();
            if selected.is_empty() {
                format!("Error: No lines to read in '{path}' at offset {from}")
            } else {
                format!(
                    "({} lines from line {from})\n{}",
                    selected.len(),
                    selected.join("\n")
                )
            }
        }
        _ => {
            let count = rendered.lines().count();
            if !notes.is_empty() {
                notes.insert(0, format!("{count} lines"));
            }
            let meta = format!("({})", notes.join(", "));
            format!("{meta}\n{rendered}")
        }
    }
}

/// Replace every binary (image / PDF) payload in a parsed notebook with a
/// placeholder. Returns the number of payloads stripped.
fn strip_binary_payloads(value: &mut Value) -> usize {
    fn is_binary_key(key: &str) -> bool {
        key.starts_with("image/") || key == "application/pdf"
    }

    /// Base64 payloads are stored either as one string or as a list of
    /// strings; both collapse to the same character count.
    fn payload_len(value: &Value) -> usize {
        match value {
            Value::String(s) => s.chars().count(),
            Value::Array(items) => items
                .iter()
                .map(|i| i.as_str().map(str::chars).map_or(0, Iterator::count))
                .sum(),
            _ => 0,
        }
    }

    match value {
        Value::Object(map) => {
            let mut stripped = 0;
            for (key, entry) in map.iter_mut() {
                if is_binary_key(key) {
                    let size = payload_len(entry);
                    if size > 0 {
                        *entry =
                            Value::String(format!("[stripped {key} output — {size} base64 chars]"));
                        stripped += 1;
                        continue;
                    }
                }
                stripped += strip_binary_payloads(entry);
            }
            stripped
        }
        Value::Array(items) => items.iter_mut().map(strip_binary_payloads).sum(),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => 0,
    }
}

/// Truncate text values longer than [`MAX_NOTEBOOK_VALUE_CHARS`]. Returns the
/// number of values truncated.
fn truncate_oversized_strings(value: &mut Value) -> usize {
    fn cap(s: &str) -> Option<String> {
        let total = s.chars().count();
        if total <= MAX_NOTEBOOK_VALUE_CHARS {
            return None;
        }
        let head = s.floor_char_boundary(MAX_NOTEBOOK_VALUE_CHARS);
        Some(format!("{}…[truncated, {total} chars total]", &s[..head]))
    }

    match value {
        Value::Object(map) => map.values_mut().map(truncate_oversized_strings).sum(),
        Value::Array(items) => items.iter_mut().map(truncate_oversized_strings).sum(),
        Value::String(s) => match cap(s) {
            Some(replacement) => {
                *s = replacement;
                1
            }
            None => 0,
        },
        Value::Null | Value::Bool(_) | Value::Number(_) => 0,
    }
}

/// Line-oriented fallback for documents that are not valid JSON: keep the
/// text but cut any line longer than [`MAX_SCRUBBED_LINE_CHARS`], which is
/// where embedded base64 lives in a notebook.
fn scrub_huge_lines(content: &str) -> String {
    content
        .lines()
        .map(|line| {
            if line.chars().count() > MAX_SCRUBBED_LINE_CHARS {
                let head = line.floor_char_boundary(MAX_SCRUBBED_LINE_CHARS);
                format!("{}…[line truncated]", &line[..head])
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
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
        format!("({} lines from line {})\n{}", lines_read, from, content)
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

    // ── notebook handling ────────────────────────────────────────────────

    /// A base64 PNG payload of the requested length, as jupyter stores it.
    fn fake_b64(len: usize) -> String {
        "A".repeat(len)
    }

    fn write_notebook(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        path
    }

    fn read_nb(path: &std::path::Path) -> String {
        read_notebook(&path.to_string_lossy(), None, None)
    }

    #[test]
    fn is_notebook_file_matches_extension() {
        assert!(is_notebook_file("lab/main.ipynb"));
        assert!(is_notebook_file("MAIN.IPYNB"));
        assert!(!is_notebook_file("main.py"));
        assert!(!is_notebook_file("nb.ipynb.bak"));
    }

    #[test]
    fn notebook_image_outputs_are_stripped_but_source_kept() {
        let dir = tempfile::tempdir().unwrap();
        let big = fake_b64(100_000);
        let nb = format!(
            r#"{{
 "cells": [
  {{"cell_type": "code", "execution_count": 12, "metadata": {{}},
   "outputs": [
    {{"name": "stdout", "output_type": "stream", "text": ["loading done\n"]}},
    {{"output_type": "display_data", "metadata": {{}},
     "data": {{"image/png": "{big}", "text/plain": ["<Figure size 640x480>"]}}}}
   ],
   "source": ["import cv2\n", "cv2.imshow('x', img)\n"]}}
 ],
 "metadata": {{}}, "nbformat": 4, "nbformat_minor": 5
}}"#
        );
        let path = write_notebook(dir.path(), "main.ipynb", &nb);
        let out = read_nb(&path);

        // The base64 blob is gone, replaced by a size-recording placeholder.
        assert!(!out.contains(&fake_b64(1000)), "base64 payload survived");
        assert!(out.contains("[stripped image/png output — 100000 base64 chars]"));

        // Everything the model actually needs survives.
        assert!(out.contains("import cv2"));
        assert!(out.contains("cv2.imshow"));
        assert!(out.contains("loading done"));
        assert!(out.contains("<Figure size 640x480>"));
        assert!(out.contains("\"execution_count\": 12"));

        // Meta line reports the line count first so the UI card still parses it.
        let meta = out.lines().next().unwrap();
        assert!(
            meta.starts_with('(') && meta.ends_with(')'),
            "meta was: {meta}"
        );
        assert!(meta.contains(" lines"), "meta was: {meta}");
        assert!(
            meta.contains("1 image outputs stripped"),
            "meta was: {meta}"
        );
    }

    #[test]
    fn notebook_image_payload_stored_as_string_list_is_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let nb = format!(
            r#"{{"cells": [{{"cell_type": "code", "outputs": [{{"output_type": "display_data",
             "data": {{"image/png": ["{}", "{}"]}}}}], "source": ["x=1"]}}],
             "nbformat": 4, "nbformat_minor": 5}}"#,
            fake_b64(600),
            fake_b64(600)
        );
        let path = write_notebook(dir.path(), "listed.ipynb", &nb);
        let out = read_nb(&path);
        assert!(out.contains("[stripped image/png output — 1200 base64 chars]"));
        assert!(!out.contains(&fake_b64(600)));
    }

    #[test]
    fn notebook_markdown_attachments_are_stripped() {
        let dir = tempfile::tempdir().unwrap();
        let nb = format!(
            r#"{{"cells": [{{"cell_type": "markdown", "attachments": {{"image/jpeg": "{b}"}},
             "source": ["see figure"]}}], "nbformat": 4, "nbformat_minor": 5}}"#,
            b = fake_b64(5000)
        );
        let path = write_notebook(dir.path(), "attach.ipynb", &nb);
        let out = read_nb(&path);
        assert!(out.contains("[stripped image/jpeg output"));
        assert!(out.contains("see figure"));
        assert!(!out.contains(&fake_b64(5000)));
    }

    #[test]
    fn notebook_oversized_text_output_is_truncated() {
        let dir = tempfile::tempdir().unwrap();
        let big = fake_b64(MAX_NOTEBOOK_VALUE_CHARS * 3);
        let nb = format!(
            r#"{{"cells": [{{"cell_type": "code", "outputs": [{{"output_type": "execute_result",
             "data": {{"text/plain": "{big}"}}}}], "source": ["df"]}}],
             "nbformat": 4, "nbformat_minor": 5}}"#
        );
        let path = write_notebook(dir.path(), "text.ipynb", &nb);
        let out = read_nb(&path);
        assert!(out.contains(&format!(
            "…[truncated, {} chars total]",
            MAX_NOTEBOOK_VALUE_CHARS * 3
        )));
        assert!(out.contains("1 oversized outputs truncated"));
        assert!(!out.contains(&big));
        assert!(out.len() < MAX_NOTEBOOK_VALUE_CHARS * 2);
    }

    #[test]
    fn notebook_without_big_outputs_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let nb = r#"{"cells": [{"cell_type": "code", "execution_count": 1,
          "outputs": [{"output_type": "stream", "name": "stdout", "text": ["hi\n"]}],
          "source": ["print('hi')"]}], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#;
        let path = write_notebook(dir.path(), "clean.ipynb", nb);
        let out = read_nb(&path);
        assert!(out.contains("print('hi')"));
        assert!(out.contains("hi\\n"));
        // Nothing was rewritten, so no action notes are appended.
        let meta = out.lines().next().unwrap();
        assert!(!meta.contains("stripped"), "meta was: {meta}");
        assert!(!meta.contains("truncated"), "meta was: {meta}");
    }

    #[test]
    fn malformed_notebook_falls_back_to_line_scrubbing() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!(
            "{{{{ not json at all\nsecond line ok\nlong line: {}\nfourth\n",
            fake_b64(MAX_SCRUBBED_LINE_CHARS * 4)
        );
        let path = write_notebook(dir.path(), "broken.ipynb", &body);
        let out = read_nb(&path);
        assert!(out.contains("second line ok"));
        assert!(out.contains("[line truncated]"));
        assert!(!out.contains(&fake_b64(MAX_SCRUBBED_LINE_CHARS * 2)));
        assert!(out.lines().next().unwrap().contains("long lines truncated"));
    }

    #[test]
    fn notebook_from_to_pages_the_rendered_text() {
        let dir = tempfile::tempdir().unwrap();
        let nb = format!(
            r#"{{"cells": [{{"cell_type": "code", "outputs": [{{"output_type": "display_data",
             "data": {{"image/png": "{b}"}}}}], "source": ["alpha\n", "beta"]}}],
             "nbformat": 4, "nbformat_minor": 5}}"#,
            b = fake_b64(20_000)
        );
        let path = write_notebook(dir.path(), "paged.ipynb", &nb);
        let full = read_notebook(&path.to_string_lossy(), None, None);
        // `from`/`to` index the rendered body, never the meta line — the same
        // contract `read_partial` honours for plain files.
        let body: Vec<&str> = full.lines().skip(1).collect();
        assert!(body.len() > 4, "fixture should render several lines");

        let window = read_notebook(&path.to_string_lossy(), Some(0), Some(3));
        assert!(window.starts_with("(3 lines from line 0)"));
        assert_eq!(window.lines().skip(1).collect::<Vec<_>>(), &body[..3]);

        let middle = read_notebook(&path.to_string_lossy(), Some(1), Some(2));
        assert_eq!(middle.lines().skip(1).collect::<Vec<_>>(), &body[1..3]);

        let tail = read_notebook(&path.to_string_lossy(), Some(body.len() - 2), Some(5));
        assert_eq!(
            tail.lines().skip(1).collect::<Vec<_>>(),
            &body[body.len() - 2..]
        );
        assert!(tail.contains("\"nbformat_minor\": 5"));

        let empty = read_notebook(&path.to_string_lossy(), Some(body.len() + 10), Some(5));
        assert!(empty.starts_with("Error: No lines to read"));
    }

    /// Regression for the bug that motivated this code path: a notebook whose
    /// cells embed four ~380 KB figures must not produce a multi-megabyte tool
    /// result (that request exceeded the provider's input limit).
    #[test]
    fn realistic_notebook_with_many_figures_stays_small() {
        let dir = tempfile::tempdir().unwrap();
        let cells: Vec<String> = (0..4)
            .map(|i| {
                format!(
                    r#"{{"cell_type": "code", "execution_count": {i}, "metadata": {{}},
                     "outputs": [{{"output_type": "display_data", "metadata": {{}},
                      "data": {{"image/png": "{b}", "text/plain": ["<Figure {i}>"]}}}}],
                     "source": ["cell_{i}\n", "imshow(x)\n"]}}"#,
                    b = fake_b64(377_440)
                )
            })
            .collect();
        let nb = format!(
            r#"{{"cells": [{}], "metadata": {{}}, "nbformat": 4, "nbformat_minor": 5}}"#,
            cells.join(",")
        );
        assert!(nb.len() > 1_400_000, "fixture should mirror the real file");

        let path = write_notebook(dir.path(), "big.ipynb", &nb);
        let out = read_nb(&path);
        assert!(out.len() < 4_000, "notebook render was {} chars", out.len());
        assert!(out.contains("cell_0") && out.contains("cell_3"));
        assert!(out.contains("4 image outputs stripped"));
    }

    #[test]
    fn read_tool_dispatches_notebooks_without_image_marker() {
        let dir = tempfile::tempdir().unwrap();
        let nb = format!(
            r#"{{"cells": [{{"cell_type": "code", "outputs": [{{"data": {{"image/png": "{b}"}}}}],
             "source": ["plot()"]}}], "nbformat": 4, "nbformat_minor": 5}}"#,
            b = fake_b64(50_000)
        );
        let path = write_notebook(dir.path(), "dispatch.ipynb", &nb);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let out = runtime.block_on(read_tool(HashMap::from([(
            "path".to_string(),
            path.to_string_lossy().to_string(),
        )])));

        // A notebook is not an image: no `[IMAGE]` marker, so the agent layer
        // must not try to attach it as a multimodal payload.
        assert!(!out.starts_with("[IMAGE]"));
        assert!(out.contains("plot()"));
        assert!(out.contains("[stripped image/png output"));
    }
}
