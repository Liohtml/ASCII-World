//! Built-in MCP (Model Context Protocol) server over stdio.
//!
//! `ascii-world mcp` speaks newline-delimited JSON-RPC 2.0 and exposes one
//! tool, `image_to_ascii`, so any MCP client (Claude Code, Cursor, Zed,
//! custom agents) can turn images into ASCII without shelling out.
//!
//! Deliberately dependency-free: the protocol surface we need is small
//! enough that hand-rolled JSON-RPC keeps the binary lean and auditable.

use crate::{charset, engine, input, render};
use anyhow::Result;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];

/// Run the stdio server until stdin closes.
pub fn serve() -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let resp = error_response(Value::Null, -32700, &format!("parse error: {e}"));
                writeln!(stdout, "{resp}")?;
                stdout.flush()?;
                continue;
            }
        };
        // Notifications (no id) get no response; messages with an id but no
        // method are client *responses* — JSON-RPC says ignore, not error.
        let Some(id) = msg.get("id").cloned() else {
            continue;
        };
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            continue;
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let response = handle(id, method, &params);
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }
    Ok(())
}

fn handle(id: Value, method: &str, params: &Value) -> Value {
    match method {
        "initialize" => {
            // Accept the client's version if we support it; otherwise offer
            // our latest so the client can downgrade or disconnect.
            let version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .filter(|v| SUPPORTED_VERSIONS.contains(v))
                .unwrap_or(PROTOCOL_VERSION);
            ok_response(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": {
                        "name": "ascii-world",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }),
            )
        }
        "ping" => ok_response(id, json!({})),
        "tools/list" => ok_response(id, json!({ "tools": [tool_schema()] })),
        "tools/call" => tools_call(id, params),
        _ => error_response(id, -32601, &format!("method not found: {method}")),
    }
}

fn tool_schema() -> Value {
    json!({
        "name": "image_to_ascii",
        "description": "Convert an image file (PNG/JPEG/GIF/WebP/BMP/SVG) to ASCII art. \
            Transparent areas render as blank, so logos stay cutouts. \
            Returns the ASCII text; optionally includes per-cell colors as JSON.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Path to the input image file"
                },
                "width": {
                    "type": "integer",
                    "description": "Output width in characters (default 100)",
                    "minimum": 1
                },
                "charset": {
                    "type": "string",
                    "description": format!(
                        "Character set: one of {NAMED:?} or 'custom:<chars dark→light>' (default 'complex'). \
                         'braille' samples a 2x4 dot grid per cell for ~8x the detail.",
                        NAMED = charset::NAMED
                    )
                },
                "invert": {
                    "type": "boolean",
                    "description": "Invert brightness mapping (for light backgrounds)"
                },
                "alpha_threshold": {
                    "type": "integer",
                    "description": "Cells whose mean alpha is below this render blank (0-255, default 64)",
                    "minimum": 0,
                    "maximum": 255
                },
                "json": {
                    "type": "boolean",
                    "description": "Return structured JSON (lines + #rrggbb colors) instead of plain text"
                }
            },
            "required": ["path"]
        }
    })
}

fn tools_call(id: Value, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    if name != "image_to_ascii" {
        return error_response(id, -32602, &format!("unknown tool: {name}"));
    }
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    match run_image_to_ascii(&args) {
        Ok(text) => ok_response(
            id,
            json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
        ),
        Err(e) => ok_response(
            id,
            json!({ "content": [{ "type": "text", "text": format!("error: {e:#}") }], "isError": true }),
        ),
    }
}

fn run_image_to_ascii(args: &Value) -> Result<String> {
    let path = args
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing required argument 'path'"))?;
    // Validate rather than silently default: agents must learn when a
    // parameter was rejected instead of getting wrong-shaped output.
    let width = match args.get("width") {
        None => 100,
        Some(v) => match v.as_u64() {
            Some(w @ 1..=100_000) => w as u32,
            _ => anyhow::bail!("'width' must be an integer between 1 and 100000, got {v}"),
        },
    };
    let charset_name = args
        .get("charset")
        .and_then(Value::as_str)
        .unwrap_or("complex");
    let invert = args.get("invert").and_then(Value::as_bool).unwrap_or(false);
    let as_json = args.get("json").and_then(Value::as_bool).unwrap_or(false);
    let alpha_threshold = match args.get("alpha_threshold") {
        None => engine::DEFAULT_ALPHA_THRESHOLD,
        Some(v) => match v.as_u64() {
            Some(a @ 0..=255) => a as u8,
            _ => anyhow::bail!("'alpha_threshold' must be an integer between 0 and 255, got {v}"),
        },
    };

    let charset = charset::resolve(charset_name)?;
    let bytes = std::fs::read(path).map_err(|e| anyhow::anyhow!("failed to open '{path}': {e}"))?;
    let img = input::decode_still(&bytes, input::svg_target_px(width))?;
    let grid = engine::convert(
        &img,
        &engine::Options {
            width,
            charset: charset.clone(),
            invert,
            aspect: 2.0,
            alpha_threshold,
            // Agents paste this into chat or a terminal; match the txt path.
            matte: Some(if invert { [0, 0, 0] } else { [255, 255, 255] }),
            braille_threshold: None,
        },
    )?;
    Ok(if as_json {
        render::to_json(&grid, &charset, invert, true)
    } else {
        render::to_text(&grid)
    })
}

fn ok_response(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_reports_server_info() {
        let resp = handle(
            json!(1),
            "initialize",
            &json!({"protocolVersion": "2025-03-26"}),
        );
        assert_eq!(resp["result"]["serverInfo"]["name"], "ascii-world");
        assert_eq!(resp["result"]["protocolVersion"], "2025-03-26");
    }

    #[test]
    fn tools_list_exposes_image_to_ascii() {
        let resp = handle(json!(2), "tools/list", &Value::Null);
        assert_eq!(resp["result"]["tools"][0]["name"], "image_to_ascii");
    }

    #[test]
    fn unknown_method_errors() {
        let resp = handle(json!(3), "nope", &Value::Null);
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[test]
    fn tool_call_with_missing_path_is_tool_error() {
        let resp = handle(
            json!(4),
            "tools/call",
            &json!({"name": "image_to_ascii", "arguments": {}}),
        );
        assert_eq!(resp["result"]["isError"], true);
    }
}
