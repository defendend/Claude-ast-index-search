//! Desktop automation via companion app (JSON-RPC over stdin/stdout)

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use anyhow::{Result, Context, bail};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use base64::Engine as _;

static REQUEST_ID: AtomicU64 = AtomicU64::new(1);

fn get_companion_path(companion_path: Option<&str>) -> Result<String> {
    if let Some(p) = companion_path {
        return Ok(p.to_string());
    }
    if let Ok(p) = std::env::var("MOBILE_TOOLS_COMPANION") {
        return Ok(p);
    }
    bail!("Desktop companion path not set. Use --companion-path or MOBILE_TOOLS_COMPANION env var")
}

/// Send JSON-RPC 2.0 request to companion and get response
fn rpc_call(companion_path: &str, method: &str, params: Value) -> Result<Value> {
    let id = REQUEST_ID.fetch_add(1, Ordering::SeqCst);
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    });

    let mut child = Command::new(companion_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to start desktop companion app")?;

    let stdin = child.stdin.as_mut().context("Failed to open stdin")?;
    let request_str = serde_json::to_string(&request)?;
    writeln!(stdin, "{}", request_str)?;
    drop(child.stdin.take());

    let stdout = child.stdout.take().context("Failed to open stdout")?;
    let reader = BufReader::new(stdout);

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response: Value = serde_json::from_str(&line)?;
        if let Some(error) = response.get("error") {
            bail!("Companion error: {}", error);
        }
        child.wait()?;
        return Ok(response["result"].clone());
    }

    let status = child.wait()?;
    if !status.success() {
        bail!("Companion exited with status: {}", status);
    }
    bail!("No response from companion")
}

pub fn screenshot(companion_path: Option<&str>) -> Result<Vec<u8>> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "screenshot", json!({}))?;
    let b64 = result["base64"].as_str().context("No base64 in response")?;
    let data = base64::engine::general_purpose::STANDARD.decode(b64)?;
    Ok(data)
}

pub fn tap(x: i32, y: i32, companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "tap", json!({"x": x, "y": y}))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn tap_by_text(text: &str, companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "tap_by_text", json!({"text": text}))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn input_text(text: &str, companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "input_text", json!({"text": text}))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn press_key(key: &str, companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "press_key", json!({"key": key}))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn get_ui(companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "get_ui", json!({}))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn launch_app(app_path: &str, companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "launch_app", json!({"app_path": app_path}))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn stop_app(app_name: &str, companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "stop_app", json!({"app_name": app_name}))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn get_window_info(companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "get_window_info", json!({}))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn focus_window(window_id: &str, companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "focus_window", json!({"window_id": window_id}))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn resize_window(window_id: &str, width: u32, height: u32, companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "resize_window", json!({
        "window_id": window_id,
        "width": width,
        "height": height
    }))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn get_clipboard(companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "get_clipboard", json!({}))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn set_clipboard(text: &str, companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "set_clipboard", json!({"text": text}))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn get_performance_metrics(companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "get_performance_metrics", json!({}))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

pub fn get_monitors(companion_path: Option<&str>) -> Result<()> {
    let path = get_companion_path(companion_path)?;
    let result = rpc_call(&path, "get_monitors", json!({}))?;
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
