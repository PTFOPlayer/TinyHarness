use std::collections::HashMap;

use crate::config::Settings;
use crate::tools::tool::{BoxFuture, Tool, build_string_params_schema, make_tool, require_arg};

/// Load the Ollama API key from settings, returning an error string if not set.
fn get_api_key() -> Result<String, String> {
    let settings = Settings::load();
    settings
        .ollama_api_key
        .ok_or_else(|| "Error: No Ollama API key set. Use /apikey <key> to set one.".to_string())
}

fn web_search_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let query = match require_arg(&args, "query") {
            Ok(q) => q,
            Err(e) => return e,
        };

        let api_key = match get_api_key() {
            Ok(k) => k,
            Err(e) => return e,
        };

        let max_results = args
            .get("max_results")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(5)
            .min(10);

        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "query": query,
            "max_results": max_results,
        });

        let resp = match client
            .post("https://ollama.com/api/web_search")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return format!("Error: Web search request failed: {}", e),
        };

        let json: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => return format!("Error: Failed to parse response: {}", e),
        };

        let results = match json.get("results").and_then(|r| r.as_array()) {
            Some(r) => r,
            None => return "No results found.".to_string(),
        };

        if results.is_empty() {
            return "No results found.".to_string();
        }

        let mut output = String::new();
        for (i, result) in results.iter().enumerate() {
            let title = result
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("(no title)");
            let url = result
                .get("url")
                .and_then(|u| u.as_str())
                .unwrap_or("(no url)");
            let content = result
                .get("content")
                .and_then(|c| c.as_str())
                .unwrap_or("(no content)");

            output.push_str(&format!(
                "[{}] {}\n    URL: {}\n    {}\n\n",
                i + 1,
                title,
                url,
                content
            ));
        }

        output
    })
}

fn web_fetch_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let url = match require_arg(&args, "url") {
            Ok(u) => u,
            Err(e) => return e,
        };

        let api_key = match get_api_key() {
            Ok(k) => k,
            Err(e) => return e,
        };

        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "url": url,
        });

        let resp = match client
            .post("https://ollama.com/api/web_fetch")
            .header("Authorization", format!("Bearer {}", api_key))
            .json(&body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => return format!("Error: Web fetch request failed: {}", e),
        };

        let json: serde_json::Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => return format!("Error: Failed to parse response: {}", e),
        };

        let title = json
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("(no title)");
        let content = json
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("(no content)");
        let links = json
            .get("links")
            .and_then(|l| l.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>()
                    .join("\n  ")
            })
            .unwrap_or_default();

        let mut output = format!("Title: {}\n\nContent:\n{}\n", title, content);
        if !links.is_empty() {
            output.push_str(&format!("\nLinks:\n  {}", links));
        }

        output
    })
}

pub fn web_search_tool_entry() -> Tool {
    make_tool(
        "web_search",
        "Search the web using Ollama's web search API. Returns relevant search results with titles, URLs, and content snippets. Use this to get up-to-date information from the internet.",
        build_string_params_schema(
            &[("query", "The search query string")],
            &[(
                "max_results",
                "Maximum number of results to return (default 5, max 10)",
                "5",
            )],
        ),
        web_search_tool,
    )
}

pub fn web_fetch_tool_entry() -> Tool {
    make_tool(
        "web_fetch",
        "Fetch the content of a specific web page by URL using Ollama's web fetch API. Returns the page title, main content, and links found on the page.",
        build_string_params_schema(&[("url", "The URL to fetch")], &[]),
        web_fetch_tool,
    )
}
