use std::io::Write;

use tinyharness_lib::image::{ImageAttachment, MAX_IMAGE_BYTES};
use tinyharness_ui::style::*;

use crate::commands::registry::CommandContext;

/// Maximum number of images that can be attached at once.
const MAX_PENDING_IMAGES: usize = 10;

/// Execute the `/image` command.
///
/// Usage:
///   `/image <path>`        — Attach an image file
///   `/image`               — Show pending images
///   `/image clear`         — Clear all pending images
///   `/image drop <n>`      — Remove a specific pending image by index
pub fn execute(ctx: &mut CommandContext, arg: Option<&str>) {
    match arg {
        None | Some("") => {
            show_pending(&ctx.pending_images, &mut ctx.output);
        }
        Some("clear") => {
            let count = ctx.pending_images.len();
            ctx.pending_images.clear();
            let _ = writeln!(
                ctx.output,
                "{GREEN}✓ Cleared {count} pending image(s).{RESET}"
            );
        }
        Some(arg) if arg.starts_with("drop ") => {
            let idx_str = arg.strip_prefix("drop ").unwrap().trim();
            match idx_str.parse::<usize>() {
                Ok(idx) if idx > 0 && idx <= ctx.pending_images.len() => {
                    let removed = ctx.pending_images.remove(idx - 1);
                    let _ = writeln!(
                        ctx.output,
                        "{GREEN}✓ Removed: {BOLD}{}{RESET}",
                        removed.display_name()
                    );
                    show_pending_compact(&ctx.pending_images, &mut ctx.output);
                }
                _ => {
                    let _ = writeln!(
                        ctx.output,
                        "{RED}Invalid index. Use /image to see the list.{RESET}"
                    );
                }
            }
        }
        Some(path) => {
            attach_image(ctx, path);
        }
    }
}

/// Attach an image from the given path.
fn attach_image(ctx: &mut CommandContext, path_str: &str) {
    if ctx.pending_images.len() >= MAX_PENDING_IMAGES {
        let _ = writeln!(
            ctx.output,
            "{RED}Cannot attach more than {} images. Use /image drop <n> to remove one first.{RESET}",
            MAX_PENDING_IMAGES
        );
        return;
    }

    // Expand tilde
    let expanded = if path_str.starts_with('~') {
        if let Ok(home) = std::env::var("HOME") {
            path_str.replacen('~', &home, 1)
        } else {
            path_str.to_string()
        }
    } else {
        path_str.to_string()
    };

    match ImageAttachment::load_from_str(&expanded) {
        Ok(img) => {
            let name = img.display_name();
            let size = img.size_display();
            ctx.pending_images.push(img);
            let _ = writeln!(
                ctx.output,
                "{GREEN}✓ Attached: {BOLD}{name}{RESET}{GREEN} ({size}){RESET}",
            );
            let count = ctx.pending_images.len();
            if count > 1 {
                let _ = writeln!(
                    ctx.output,
                    "  {DIM}({count} image(s) pending — will be sent with your next message){RESET}"
                );
            } else {
                let _ = writeln!(
                    ctx.output,
                    "  {DIM}(Image will be sent with your next message){RESET}"
                );
            }
        }
        Err(e) => {
            let _ = writeln!(ctx.output, "{RED}{e}{RESET}");
            // Give a hint about supported formats
            let _ = writeln!(
                ctx.output,
                "  {DIM}Supported formats: png, jpg/jpeg, webp, gif, bmp. Max size: {} MB.{RESET}",
                MAX_IMAGE_BYTES / (1024 * 1024)
            );
        }
    }
}

/// Show all pending images with details.
fn show_pending(images: &[ImageAttachment], stdout: &mut impl Write) {
    if images.is_empty() {
        let _ = writeln!(
            stdout,
            "{DIM}No pending images. Use {BOLD}/image <path>{DIM} to attach one.{RESET}"
        );
        return;
    }

    let _ = writeln!(
        stdout,
        "{BOLD}Pending images ({count}):{RESET}",
        count = images.len()
    );
    for (i, img) in images.iter().enumerate() {
        let name = img.display_name();
        let size = img.size_display();
        let path = img.path.display();
        let _ = writeln!(
            stdout,
            "  {BOLD}{}.{RESET} {CYAN}{name}{RESET} {DIM}({size}){RESET}",
            i + 1
        );
        let _ = writeln!(stdout, "     {DIM}{path}{RESET}");
    }
    let _ = writeln!(
        stdout,
        "\n{DIM}Use {BOLD}/image clear{DIM} to remove all, or {BOLD}/image drop <n>{DIM} to remove one.{RESET}"
    );
}

/// Compact one-line listing of pending images (after a drop).
fn show_pending_compact(images: &[ImageAttachment], stdout: &mut impl Write) {
    if images.is_empty() {
        let _ = writeln!(stdout, "{DIM}No pending images.{RESET}");
    } else {
        let names: Vec<String> = images.iter().map(|img| img.display_name()).collect();
        let _ = writeln!(stdout, "{DIM}Pending: {GRAY}{}{RESET}", names.join(", "));
    }
}
