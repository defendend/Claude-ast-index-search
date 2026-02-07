//! Android device automation via ADB

use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;
use anyhow::{Result, Context, bail};
use regex::Regex;
use serde::Serialize;

// Compiled regexes (created once, reused)
fn node_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"<node\s+[^>]+>"#).unwrap())
}

fn class_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"class="([^"]*)""#).unwrap())
}

fn text_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"text="([^"]*)""#).unwrap())
}

fn resource_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"resource-id="([^"]*)""#).unwrap())
}

fn content_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"content-desc="([^"]*)""#).unwrap())
}

fn bounds_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"bounds="\[(\d+),(\d+)\]\[(\d+),(\d+)\]""#).unwrap())
}

fn bounds_string_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"bounds="([^"]*)""#).unwrap())
}

fn clickable_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"clickable="([^"]*)""#).unwrap())
}

/// Build ADB command with optional device serial
fn adb_cmd(device: Option<&str>) -> Command {
    let mut cmd = Command::new("adb");
    if let Some(serial) = device {
        cmd.arg("-s").arg(serial);
    }
    cmd
}

/// Execute ADB command with timeout
fn adb_exec(device: Option<&str>, args: &[&str], timeout: Option<Duration>) -> Result<std::process::Output> {
    let mut cmd = adb_cmd(device);
    cmd.args(args);

    if let Some(_t) = timeout {
        // For now, just execute without timeout
        // Full timeout support would require tokio or similar
        cmd.output().context("Failed to execute adb command")
    } else {
        cmd.output().context("Failed to execute adb command")
    }
}

/// Take screenshot and return PNG bytes
pub fn screenshot(device: Option<&str>) -> Result<Vec<u8>> {
    let output = adb_exec(device, &["exec-out", "screencap", "-p"], None)?;

    if !output.status.success() {
        bail!("adb screencap failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(output.stdout)
}

/// Tap at coordinates
pub fn tap(x: i32, y: i32, device: Option<&str>) -> Result<()> {
    let output = adb_exec(device, &["shell", "input", "tap", &x.to_string(), &y.to_string()], None)?;

    if !output.status.success() {
        bail!("adb tap failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Tapped at ({}, {})", x, y);
    Ok(())
}

/// Long press at coordinates
pub fn long_press(x: i32, y: i32, duration: u32, device: Option<&str>) -> Result<()> {
    let output = adb_exec(device, &[
        "shell", "input", "swipe",
        &x.to_string(), &y.to_string(),
        &x.to_string(), &y.to_string(),
        &duration.to_string(),
    ], None)?;

    if !output.status.success() {
        bail!("adb long press failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Long pressed at ({}, {}) for {}ms", x, y, duration);
    Ok(())
}

/// Open URL in default browser
pub fn open_url(url: &str, device: Option<&str>) -> Result<()> {
    let output = adb_exec(device, &["shell", "am", "start", "-a", "android.intent.action.VIEW", "-d", url], None)?;

    if !output.status.success() {
        bail!("Failed to open URL: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Opened URL: {}", url);
    Ok(())
}

/// Execute shell command on device
pub fn shell(command: &str, device: Option<&str>) -> Result<String> {
    let output = adb_exec(device, &["shell", command], None)?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() && !stderr.is_empty() {
        eprintln!("{}", stderr);
    }

    print!("{}", stdout);
    Ok(stdout)
}

/// Swipe gesture
pub fn swipe(x1: i32, y1: i32, x2: i32, y2: i32, duration: u32, device: Option<&str>) -> Result<()> {
    let output = adb_exec(device, &[
        "shell", "input", "swipe",
        &x1.to_string(), &y1.to_string(),
        &x2.to_string(), &y2.to_string(),
        &duration.to_string(),
    ], None)?;

    if !output.status.success() {
        bail!("adb swipe failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Swiped from ({}, {}) to ({}, {})", x1, y1, x2, y2);
    Ok(())
}

/// Input text (with proper escaping)
pub fn input_text(text: &str, device: Option<&str>) -> Result<()> {
    // Escape special characters for shell
    let escaped = text
        .replace('\\', "\\\\")
        .replace(' ', "%s")
        .replace('\'', "\\'")
        .replace('"', "\\\"")
        .replace('&', "\\&")
        .replace('|', "\\|")
        .replace(';', "\\;")
        .replace('$', "\\$")
        .replace('`', "\\`");

    let output = adb_exec(device, &["shell", "input", "text", &escaped], None)?;

    if !output.status.success() {
        bail!("adb input text failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Input text: {}", text);
    Ok(())
}

/// Press a key
pub fn press_key(key: &str, device: Option<&str>) -> Result<()> {
    let keycode = match key.to_lowercase().as_str() {
        "home" => "KEYCODE_HOME",
        "back" => "KEYCODE_BACK",
        "enter" | "return" => "KEYCODE_ENTER",
        "tab" => "KEYCODE_TAB",
        "delete" | "backspace" => "KEYCODE_DEL",
        "menu" => "KEYCODE_MENU",
        "power" => "KEYCODE_POWER",
        "volume_up" => "KEYCODE_VOLUME_UP",
        "volume_down" => "KEYCODE_VOLUME_DOWN",
        "camera" => "KEYCODE_CAMERA",
        "search" => "KEYCODE_SEARCH",
        "space" => "KEYCODE_SPACE",
        "escape" | "esc" => "KEYCODE_ESCAPE",
        "up" => "KEYCODE_DPAD_UP",
        "down" => "KEYCODE_DPAD_DOWN",
        "left" => "KEYCODE_DPAD_LEFT",
        "right" => "KEYCODE_DPAD_RIGHT",
        "app_switch" | "recent" => "KEYCODE_APP_SWITCH",
        _ => key,
    };

    let output = adb_exec(device, &["shell", "input", "keyevent", keycode], None)?;

    if !output.status.success() {
        bail!("adb keyevent failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Pressed key: {} ({})", key, keycode);
    Ok(())
}

// ============== UI Dump (shared implementation) ==============

/// Get raw UI XML from device
fn get_ui_xml(device: Option<&str>) -> Result<String> {
    // Combined command: dump + cat + cleanup in one shell call
    let output = adb_exec(device, &[
        "shell",
        "uiautomator dump /sdcard/ui.xml >/dev/null 2>&1 && cat /sdcard/ui.xml && rm /sdcard/ui.xml"
    ], None)?;

    if !output.status.success() {
        bail!("Failed to get UI dump: {}", String::from_utf8_lossy(&output.stderr));
    }

    let xml = String::from_utf8_lossy(&output.stdout).to_string();
    if xml.is_empty() || !xml.contains("<hierarchy") {
        bail!("UI dump returned empty or invalid XML");
    }

    Ok(xml)
}

/// UI Element with parsed bounds
#[derive(Clone, Debug, Serialize)]
pub struct UiElement {
    pub class: String,
    pub text: String,
    pub resource_id: String,
    pub content_desc: String,
    pub bounds: (i32, i32, i32, i32),
    pub clickable: bool,
}

impl UiElement {
    pub fn center(&self) -> (i32, i32) {
        ((self.bounds.0 + self.bounds.2) / 2, (self.bounds.1 + self.bounds.3) / 2)
    }

    pub fn label(&self) -> String {
        if !self.text.is_empty() {
            self.text.clone()
        } else if !self.content_desc.is_empty() {
            self.content_desc.clone()
        } else if !self.resource_id.is_empty() {
            self.resource_id.split('/').last().unwrap_or("").to_string()
        } else {
            self.class.split('.').last().unwrap_or("").to_string()
        }
    }

    #[allow(dead_code)]
    pub fn width(&self) -> i32 {
        self.bounds.2 - self.bounds.0
    }

    #[allow(dead_code)]
    pub fn height(&self) -> i32 {
        self.bounds.3 - self.bounds.1
    }
}

/// Parse UI XML into elements
fn parse_ui_elements(xml: &str) -> Vec<UiElement> {
    let mut elements = Vec::new();

    for node in node_regex().find_iter(xml) {
        let node_str = node.as_str();

        let class = class_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        let text = text_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        let resource_id = resource_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        let content_desc = content_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        let bounds = if let Some(caps) = bounds_regex().captures(node_str) {
            (
                caps[1].parse().unwrap_or(0),
                caps[2].parse().unwrap_or(0),
                caps[3].parse().unwrap_or(0),
                caps[4].parse().unwrap_or(0),
            )
        } else {
            continue;
        };

        let clickable = clickable_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str() == "true")
            .unwrap_or(false);

        if !class.is_empty() && (clickable || !text.is_empty() || !content_desc.is_empty()) {
            elements.push(UiElement {
                class,
                text,
                resource_id,
                content_desc,
                bounds,
                clickable,
            });
        }
    }

    elements
}

/// Get UI elements as structured data
pub fn get_ui_elements(device: Option<&str>) -> Result<Vec<UiElement>> {
    let xml = get_ui_xml(device)?;
    Ok(parse_ui_elements(&xml))
}

/// Dump UI hierarchy
pub fn ui_dump(format: &str, device: Option<&str>) -> Result<()> {
    let xml = get_ui_xml(device)?;

    if format == "json" {
        println!("{}", xml_to_json(&xml)?);
    } else {
        println!("{}", xml);
    }

    Ok(())
}

/// Convert UI XML to simplified JSON
fn xml_to_json(xml: &str) -> Result<String> {
    #[derive(Serialize)]
    struct UiElementJson {
        class: String,
        #[serde(skip_serializing_if = "String::is_empty")]
        text: String,
        #[serde(skip_serializing_if = "String::is_empty")]
        resource_id: String,
        #[serde(skip_serializing_if = "String::is_empty")]
        content_desc: String,
        bounds: String,
        clickable: bool,
    }

    let mut elements = Vec::new();

    for node in node_regex().find_iter(xml) {
        let node_str = node.as_str();

        let class = class_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        let text = text_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        let resource_id = resource_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        let content_desc = content_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        let bounds = bounds_string_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();

        let clickable = clickable_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str() == "true")
            .unwrap_or(false);

        if !class.is_empty() && !bounds.is_empty() {
            elements.push(UiElementJson {
                class,
                text,
                resource_id,
                content_desc,
                bounds,
                clickable,
            });
        }
    }

    Ok(serde_json::to_string_pretty(&elements)?)
}

// ============== Element Finding ==============

/// Find element by text/resource-id and return center coordinates
pub fn find_element(query: &str, device: Option<&str>) -> Result<Option<(i32, i32)>> {
    let xml = get_ui_xml(device)?;
    let query_lower = query.to_lowercase();

    for node in node_regex().find_iter(&xml) {
        let node_str = node.as_str();

        let text = text_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .unwrap_or("");

        let resource_id = resource_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .unwrap_or("");

        let content_desc = content_regex().captures(node_str)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .unwrap_or("");

        let matches = text.to_lowercase().contains(&query_lower)
            || resource_id.to_lowercase().contains(&query_lower)
            || content_desc.to_lowercase().contains(&query_lower);

        if matches {
            if let Some(caps) = bounds_regex().captures(node_str) {
                let x1: i32 = caps[1].parse().unwrap_or(0);
                let y1: i32 = caps[2].parse().unwrap_or(0);
                let x2: i32 = caps[3].parse().unwrap_or(0);
                let y2: i32 = caps[4].parse().unwrap_or(0);

                let center_x = (x1 + x2) / 2;
                let center_y = (y1 + y2) / 2;

                println!("Found: text=\"{}\" resource_id=\"{}\" content_desc=\"{}\"", text, resource_id, content_desc);
                println!("Bounds: [{},{}][{},{}] -> center: ({}, {})", x1, y1, x2, y2, center_x, center_y);

                return Ok(Some((center_x, center_y)));
            }
        }
    }

    println!("Element with '{}' not found", query);
    Ok(None)
}

/// Tap element by text/resource-id
pub fn tap_element(query: &str, device: Option<&str>) -> Result<()> {
    if let Some((x, y)) = find_element(query, device)? {
        tap(x, y, device)?;
    } else {
        bail!("Element '{}' not found", query);
    }
    Ok(())
}

// ============== Device Management ==============

#[derive(Serialize)]
pub struct Device {
    pub serial: String,
    pub state: String,
    pub model: Option<String>,
}

/// List connected devices
pub fn list_devices() -> Result<Vec<Device>> {
    let output = Command::new("adb")
        .args(["devices", "-l"])
        .output()
        .context("Failed to execute adb devices")?;

    if !output.status.success() {
        bail!("adb devices failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut devices = Vec::new();

    for line in stdout.lines().skip(1) {
        if line.trim().is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let model = parts.iter()
                .find(|p| p.starts_with("model:"))
                .map(|p| p.trim_start_matches("model:").to_string());

            devices.push(Device {
                serial: parts[0].to_string(),
                state: parts[1].to_string(),
                model,
            });
        }
    }

    Ok(devices)
}

/// Print devices list
pub fn print_devices() -> Result<()> {
    let devices = list_devices()?;
    println!("Android devices:");
    println!("{}", serde_json::to_string_pretty(&devices)?);
    Ok(())
}

// ============== App Management ==============

/// List installed apps
pub fn list_apps(filter: Option<&str>, device: Option<&str>) -> Result<()> {
    let output = adb_exec(device, &["shell", "pm", "list", "packages", "-3"], None)?;

    if !output.status.success() {
        bail!("pm list packages failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut apps: Vec<String> = stdout
        .lines()
        .filter_map(|line| line.strip_prefix("package:"))
        .filter(|pkg| {
            filter.map_or(true, |f| pkg.to_lowercase().contains(&f.to_lowercase()))
        })
        .map(|s| s.to_string())
        .collect();

    apps.sort();

    println!("Installed apps ({}):", apps.len());
    for app in &apps {
        println!("  {}", app);
    }
    Ok(())
}

/// Launch an app (using am start for speed)
pub fn launch_app(package: &str, device: Option<&str>) -> Result<()> {
    // Resolve launcher activity and start in one shell call
    let cmd = format!(
        "am start -a android.intent.action.MAIN -c android.intent.category.LAUNCHER \
         $(cmd package resolve-activity --brief -c android.intent.category.LAUNCHER {} | tail -1)",
        package
    );

    let output = adb_exec(device, &["shell", &cmd], None)?;

    if !output.status.success() {
        // Fallback to monkey if resolve-activity fails (older Android)
        let fallback = adb_exec(device, &[
            "shell", "monkey", "-p", package,
            "-c", "android.intent.category.LAUNCHER", "1"
        ], None)?;

        if !fallback.status.success() {
            bail!("Failed to launch {}", package);
        }
    }

    println!("Launched: {}", package);
    Ok(())
}

/// Stop an app
pub fn stop_app(package: &str, device: Option<&str>) -> Result<()> {
    let output = adb_exec(device, &["shell", "am", "force-stop", package], None)?;

    if !output.status.success() {
        bail!("Failed to stop {}: {}", package, String::from_utf8_lossy(&output.stderr));
    }

    println!("Stopped: {}", package);
    Ok(())
}

/// Install an APK
pub fn install_app(path: &str, device: Option<&str>) -> Result<()> {
    println!("Installing {}...", path);

    let output = adb_exec(device, &["install", "-r", path], None)?;

    if !output.status.success() {
        bail!("Failed to install: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Installed: {}", path);
    Ok(())
}

/// Uninstall an app
pub fn uninstall_app(package: &str, device: Option<&str>) -> Result<()> {
    println!("Uninstalling {}...", package);

    let output = adb_exec(device, &["uninstall", package], None)?;

    if !output.status.success() {
        bail!("Failed to uninstall: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Uninstalled: {}", package);
    Ok(())
}

// ============== System Commands ==============

/// Clear device logs
pub fn clear_logs(device: Option<&str>) -> Result<()> {
    let output = adb_exec(device, &["logcat", "-c"], None)?;

    if !output.status.success() {
        bail!("logcat clear failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Logs cleared");
    Ok(())
}

/// Get system info (battery, memory)
pub fn get_system_info(device: Option<&str>) -> Result<()> {
    let battery = adb_exec(device, &["shell", "dumpsys", "battery"], None)?;
    let battery_out = String::from_utf8_lossy(&battery.stdout);

    let mut battery_level = "unknown".to_string();
    let mut battery_status = "unknown".to_string();
    for line in battery_out.lines() {
        if line.contains("level:") {
            battery_level = line.split(':').nth(1).unwrap_or("").trim().to_string();
        }
        if line.contains("status:") {
            let status_code = line.split(':').nth(1).unwrap_or("").trim();
            battery_status = match status_code {
                "1" => "Unknown",
                "2" => "Charging",
                "3" => "Discharging",
                "4" => "Not charging",
                "5" => "Full",
                _ => status_code,
            }.to_string();
        }
    }

    let meminfo = adb_exec(device, &["shell", "cat", "/proc/meminfo"], None)?;
    let mem_out = String::from_utf8_lossy(&meminfo.stdout);
    let mut mem_total = "unknown".to_string();
    let mut mem_available = "unknown".to_string();
    for line in mem_out.lines() {
        if line.starts_with("MemTotal:") {
            mem_total = line.split_whitespace().nth(1).unwrap_or("").to_string();
        }
        if line.starts_with("MemAvailable:") {
            mem_available = line.split_whitespace().nth(1).unwrap_or("").to_string();
        }
    }

    println!("System Info:");
    println!("  Battery: {}% ({})", battery_level, battery_status);
    println!("  Memory: {} kB available / {} kB total", mem_available, mem_total);

    Ok(())
}

/// Get current activity/app
pub fn get_current_activity(device: Option<&str>) -> Result<()> {
    let output = adb_exec(device, &["shell", "dumpsys", "window"], None)?;
    let out = String::from_utf8_lossy(&output.stdout);

    for line in out.lines() {
        if line.contains("mCurrentFocus") || line.contains("mFocusedApp") {
            println!("{}", line.trim());
        }
    }

    Ok(())
}

/// Get device logs
pub fn get_logs(filter: Option<&str>, lines: usize, device: Option<&str>) -> Result<()> {
    let lines_str = lines.to_string();
    let mut args = vec!["logcat", "-d", "-t", &lines_str];

    if let Some(f) = filter {
        args.push("-s");
        args.push(f);
    }

    let output = adb_exec(device, &args, None)?;

    if !output.status.success() {
        bail!("logcat failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

/// Reboot device
pub fn reboot(device: Option<&str>) -> Result<()> {
    println!("Rebooting device...");
    let output = adb_exec(device, &["reboot"], None)?;

    if !output.status.success() {
        bail!("Failed to reboot: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Reboot initiated");
    Ok(())
}

/// Turn screen on/off
pub fn screen_power(on: bool, device: Option<&str>) -> Result<()> {
    // First check screen state
    let output = adb_exec(device, &["shell", "dumpsys", "power"], None)?;
    let power_out = String::from_utf8_lossy(&output.stdout);

    let is_screen_on = power_out.contains("mWakefulness=Awake") ||
                       power_out.contains("Display Power: state=ON");

    if on && !is_screen_on {
        // Turn screen on
        adb_exec(device, &["shell", "input", "keyevent", "KEYCODE_WAKEUP"], None)?;
        println!("Screen turned ON");
    } else if !on && is_screen_on {
        // Turn screen off
        adb_exec(device, &["shell", "input", "keyevent", "KEYCODE_SLEEP"], None)?;
        println!("Screen turned OFF");
    } else {
        println!("Screen is already {}", if on { "ON" } else { "OFF" });
    }

    Ok(())
}

/// Get screen resolution
pub fn get_screen_size(device: Option<&str>) -> Result<(u32, u32)> {
    let output = adb_exec(device, &["shell", "wm", "size"], None)?;
    let out = String::from_utf8_lossy(&output.stdout);

    // Parse "Physical size: 1080x2400"
    for line in out.lines() {
        if line.contains("Physical size:") || line.contains("Override size:") {
            if let Some(size) = line.split(':').nth(1) {
                let parts: Vec<&str> = size.trim().split('x').collect();
                if parts.len() == 2 {
                    let w: u32 = parts[0].parse().unwrap_or(1080);
                    let h: u32 = parts[1].parse().unwrap_or(1920);
                    return Ok((w, h));
                }
            }
        }
    }

    Ok((1080, 1920)) // Default fallback
}

/// Analyze screen and return structured element categories
pub fn analyze_screen(device: Option<&str>) -> Result<()> {
    let elements = get_ui_elements(device)?;

    #[derive(Serialize)]
    struct ScreenAnalysis {
        buttons: Vec<ElementInfo>,
        inputs: Vec<ElementInfo>,
        texts: Vec<ElementInfo>,
        scrollable: Vec<ElementInfo>,
        images: Vec<ElementInfo>,
    }

    #[derive(Serialize)]
    struct ElementInfo {
        label: String,
        center: (i32, i32),
        bounds: (i32, i32, i32, i32),
        resource_id: String,
    }

    let mut analysis = ScreenAnalysis {
        buttons: vec![],
        inputs: vec![],
        texts: vec![],
        scrollable: vec![],
        images: vec![],
    };

    for elem in &elements {
        let info = ElementInfo {
            label: elem.label(),
            center: elem.center(),
            bounds: elem.bounds,
            resource_id: elem.resource_id.clone(),
        };

        let class_lower = elem.class.to_lowercase();
        if class_lower.contains("button") || (elem.clickable && !class_lower.contains("layout")) {
            analysis.buttons.push(info);
        } else if class_lower.contains("edittext") || class_lower.contains("input") {
            analysis.inputs.push(info);
        } else if class_lower.contains("textview") || class_lower.contains("text") {
            analysis.texts.push(info);
        } else if class_lower.contains("scroll") || class_lower.contains("recycler") || class_lower.contains("listview") {
            analysis.scrollable.push(info);
        } else if class_lower.contains("image") {
            analysis.images.push(info);
        }
    }

    println!("{}", serde_json::to_string_pretty(&analysis)?);
    Ok(())
}

/// Find element by fuzzy description and tap it
pub fn find_and_tap(description: &str, min_confidence: u32, device: Option<&str>) -> Result<()> {
    let elements = get_ui_elements(device)?;
    let desc_lower = description.to_lowercase();

    let mut best_score: u32 = 0;
    let mut best_element: Option<&UiElement> = None;

    for elem in &elements {
        let mut score: u32 = 0;
        let text_lower = elem.text.to_lowercase();
        let content_lower = elem.content_desc.to_lowercase();
        let res_lower = elem.resource_id.to_lowercase().replace('_', " ").replace('/', " ");

        // Exact text match
        if text_lower == desc_lower {
            score = score.max(100);
        }
        // Exact content-desc match
        if content_lower == desc_lower {
            score = score.max(95);
        }
        // Text contains description
        if !text_lower.is_empty() && text_lower.contains(&desc_lower) {
            score = score.max(80);
        }
        // Content-desc contains description
        if !content_lower.is_empty() && content_lower.contains(&desc_lower) {
            score = score.max(75);
        }
        // Resource-id contains description
        if !res_lower.is_empty() && res_lower.contains(&desc_lower) {
            score = score.max(60);
        }
        // Word match in text (words longer than 2 chars)
        for word in desc_lower.split_whitespace() {
            if word.len() > 2 && text_lower.contains(word) {
                score = score.max(40);
            }
            if word.len() > 2 && content_lower.contains(word) {
                score = score.max(35);
            }
        }

        // Bonus for clickable
        if score > 0 && elem.clickable {
            score = (score + 10).min(100);
        }

        if score > best_score {
            best_score = score;
            best_element = Some(elem);
        }
    }

    if best_score >= min_confidence {
        if let Some(elem) = best_element {
            let (cx, cy) = elem.center();
            println!("Found: \"{}\" (confidence: {}%)", elem.label(), best_score);
            tap(cx, cy, device)?;
            return Ok(());
        }
    }

    bail!("No element matching '{}' found with confidence >= {}%", description, min_confidence);
}

/// Push file to device
pub fn push_file(local: &str, remote: &str, device: Option<&str>) -> Result<()> {
    let output = adb_exec(device, &["push", local, remote], None)?;
    if !output.status.success() {
        bail!("adb push failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    println!("Pushed {} -> {}", local, remote);
    Ok(())
}

/// Pull file from device
pub fn pull_file(remote: &str, local: &str, device: Option<&str>) -> Result<()> {
    let output = adb_exec(device, &["pull", remote, local], None)?;
    if !output.status.success() {
        bail!("adb pull failed: {}", String::from_utf8_lossy(&output.stderr));
    }
    println!("Pulled {} -> {}", remote, local);
    Ok(())
}

/// Get clipboard content
pub fn get_clipboard(device: Option<&str>) -> Result<()> {
    let output = adb_exec(device, &["shell", "service", "call", "clipboard", "2", "s16", "com.android.shell"], None)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Parse parcel response - extract string between single quotes
    if let Some(start) = stdout.find('\'') {
        if let Some(end) = stdout[start+1..].find('\'') {
            let text = &stdout[start+1..start+1+end];
            println!("{}", text.replace("\\n", "\n"));
            return Ok(());
        }
    }
    println!("{}", stdout);
    Ok(())
}

/// Set clipboard content
pub fn set_clipboard(text: &str, device: Option<&str>) -> Result<()> {
    let cmd = format!("am broadcast -a clipper.set -e text '{}'", text.replace('\'', "'\\''"));
    let output = adb_exec(device, &["shell", &cmd], None)?;
    if !output.status.success() {
        // Fallback: try input method
        let _ = adb_exec(device, &["shell", "service", "call", "clipboard", "1", "s16", "com.android.shell", "s16", text], None)?;
    }
    println!("Clipboard set");
    Ok(())
}

// ============== Tests ==============

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ui_elements() {
        let xml = r#"
            <node class="android.widget.Button" text="Click me" bounds="[0,0][100,50]" clickable="true" resource-id="" content-desc=""/>
            <node class="android.widget.TextView" text="Hello" bounds="[0,50][200,100]" clickable="false" resource-id="com.app:id/text" content-desc=""/>
        "#;

        let elements = parse_ui_elements(xml);
        assert_eq!(elements.len(), 2);
        assert_eq!(elements[0].text, "Click me");
        assert!(elements[0].clickable);
        assert_eq!(elements[0].center(), (50, 25));
        assert_eq!(elements[1].resource_id, "com.app:id/text");
    }

    #[test]
    fn test_ui_element_label() {
        let elem = UiElement {
            class: "android.widget.Button".to_string(),
            text: "".to_string(),
            resource_id: "com.app:id/my_button".to_string(),
            content_desc: "".to_string(),
            bounds: (0, 0, 100, 50),
            clickable: true,
        };

        assert_eq!(elem.label(), "my_button");
    }

    #[test]
    fn test_regexes_compile() {
        // Ensure all regexes compile without panic
        let _ = node_regex();
        let _ = class_regex();
        let _ = text_regex();
        let _ = resource_regex();
        let _ = content_regex();
        let _ = bounds_regex();
        let _ = clickable_regex();
    }
}
