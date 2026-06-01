use std::collections::HashMap;

use crate::tools::tool::{ToolCategory, build_string_params_schema, make_tool};

pub fn screenshot_tool_entry() -> crate::tools::tool::Tool {
    make_tool(
        "screenshot",
        "Request the user to take a screenshot and attach it via /image. Use this when you need the user to show you something visually (UI element, error dialog, chart, diagram, etc.). The tool asks the user to provide a screenshot; the user can then attach an image with the /image command and reply.",
        ToolCategory::ReadOnly,
        build_string_params_schema(
            &[(
                "description",
                "What you want the user to capture (e.g. 'the error dialog', 'the main window showing the layout issue')",
            )],
            &[],
        ),
        move |args| Box::pin(screenshot_tool(args)),
    )
}

async fn screenshot_tool(args: HashMap<String, String>) -> String {
    let description = args.get("description").cloned().unwrap_or_default();

    format!(
        "Screenshot requested: '{}'\n\n\
        The user has been asked to provide a screenshot. If they attach an image using /image and reply, \
        the image will be included in their next message for you to analyze.\n\n\
        Tell the user what you need them to capture, and ask them to use /image <path> to attach it, \
        then reply so you can see it.",
        description
    )
}
