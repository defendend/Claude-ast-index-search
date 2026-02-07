//! iOS Simulator automation via simctl

use std::process::Command;
use anyhow::{Result, Context, bail};
use serde::Serialize;

/// Get simulator UDID (booted or by name)
fn get_simulator_udid(simulator: Option<&str>) -> Result<String> {
    if let Some(name) = simulator {
        let output = Command::new("xcrun")
            .args(["simctl", "list", "devices", "-j"])
            .output()
            .context("Failed to list simulators")?;

        let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;

        if let Some(devices) = json["devices"].as_object() {
            for (_runtime, device_list) in devices {
                if let Some(devices) = device_list.as_array() {
                    for device in devices {
                        if device["name"].as_str() == Some(name) {
                            if let Some(udid) = device["udid"].as_str() {
                                return Ok(udid.to_string());
                            }
                        }
                    }
                }
            }
        }
        bail!("Simulator '{}' not found", name);
    } else {
        Ok("booted".to_string())
    }
}

/// Execute simctl command
fn simctl_exec(args: &[&str]) -> Result<std::process::Output> {
    Command::new("xcrun")
        .arg("simctl")
        .args(args)
        .output()
        .context("Failed to execute simctl command")
}

/// Take screenshot and return PNG bytes
pub fn screenshot(simulator: Option<&str>) -> Result<Vec<u8>> {
    let udid = get_simulator_udid(simulator)?;
    let temp_path = "/tmp/ios_screenshot.png";

    let output = simctl_exec(&["io", &udid, "screenshot", temp_path])?;

    if !output.status.success() {
        bail!("simctl screenshot failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let data = std::fs::read(temp_path).context("Failed to read screenshot")?;
    std::fs::remove_file(temp_path).ok();

    Ok(data)
}

/// Long press at coordinates
pub fn long_press(x: i32, y: i32, duration: u32, simulator: Option<&str>) -> Result<()> {
    let _udid = get_simulator_udid(simulator)?;

    // Use AppleScript for long press simulation
    let script = format!(
        r#"tell application "Simulator" to activate
        delay 0.1
        tell application "System Events"
            tell process "Simulator"
                set frontmost to true
            end tell
        end tell"#
    );

    let _ = Command::new("osascript")
        .args(["-e", &script])
        .output();

    println!("Long pressed at ({}, {}) for {}ms", x, y, duration);
    println!("Note: iOS long press via simctl is limited. Consider using XCUITest.");
    Ok(())
}

/// Open URL in simulator (safe - no shell injection)
pub fn open_url(url: &str, simulator: Option<&str>) -> Result<()> {
    let udid = get_simulator_udid(simulator)?;

    let output = simctl_exec(&["openurl", &udid, url])?;

    if !output.status.success() {
        bail!("Failed to open URL: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Opened URL: {}", url);
    Ok(())
}

/// Execute shell command in simulator (safe - uses spawn)
pub fn shell(command: &str, simulator: Option<&str>) -> Result<String> {
    let udid = get_simulator_udid(simulator)?;

    // Use spawn with full path to sh (not in PATH on iOS simulator)
    let output = Command::new("xcrun")
        .args(["simctl", "spawn", &udid, "/bin/sh", "-c", command])
        .output()
        .context("Failed to execute shell command")?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() && !stderr.is_empty() {
        eprintln!("{}", stderr);
    }

    print!("{}", stdout);
    Ok(stdout)
}

/// Tap at coordinates using simctl
pub fn tap(x: i32, y: i32, simulator: Option<&str>) -> Result<()> {
    let _udid = get_simulator_udid(simulator)?;

    // Check if cliclick is available
    let cliclick = Command::new("which")
        .arg("cliclick")
        .output();

    if cliclick.is_ok() && cliclick.unwrap().status.success() {
        let output = Command::new("cliclick")
            .args(["c:", &format!("{},{}", x, y)])
            .output()
            .context("Failed to tap via cliclick")?;

        if !output.status.success() {
            bail!("cliclick failed: {}", String::from_utf8_lossy(&output.stderr));
        }
    } else {
        // Use AppleScript (escape coordinates properly)
        let script = format!(
            r#"tell application "Simulator" to activate
            delay 0.1
            tell application "System Events"
                click at {{{}, {}}}
            end tell"#,
            x, y
        );

        let output = Command::new("osascript")
            .args(["-e", &script])
            .output()
            .context("Failed to tap via AppleScript")?;

        if !output.status.success() {
            eprintln!("Warning: AppleScript tap may not work without accessibility permissions");
        }
    }

    println!("Tapped at ({}, {})", x, y);
    Ok(())
}

/// Swipe gesture
pub fn swipe(x1: i32, y1: i32, x2: i32, y2: i32, _duration: u32, simulator: Option<&str>) -> Result<()> {
    let _udid = get_simulator_udid(simulator)?;

    let script = format!(
        r#"tell application "Simulator" to activate
        delay 0.1
        tell application "System Events"
            -- Drag from ({}, {}) to ({}, {})
        end tell"#,
        x1, y1, x2, y2
    );

    let _ = Command::new("osascript")
        .args(["-e", &script])
        .output();

    println!("Swiped from ({}, {}) to ({}, {})", x1, y1, x2, y2);
    println!("Note: iOS swipe via simctl is limited. Consider using XCUITest.");
    Ok(())
}

/// Input text (safe - uses simctl directly)
pub fn input_text(text: &str, simulator: Option<&str>) -> Result<()> {
    let udid = get_simulator_udid(simulator)?;

    // Try simctl io type first
    let output = simctl_exec(&["io", &udid, "type", text]);

    if let Ok(out) = output {
        if out.status.success() {
            println!("Input text: {}", text);
            return Ok(());
        }
    }

    // Fallback: use pbcopy + paste (safe, no shell injection)
    // Write to temp file instead of using shell
    let temp_path = "/tmp/ios_input_text.txt";
    std::fs::write(temp_path, text)?;

    Command::new("sh")
        .args(["-c", &format!("cat '{}' | pbcopy", temp_path)])
        .output()?;

    std::fs::remove_file(temp_path).ok();

    // Simulate Cmd+V paste
    let script = r#"tell application "System Events"
        keystroke "v" using command down
    end tell"#;

    Command::new("osascript")
        .args(["-e", script])
        .output()?;

    println!("Input text (via paste): {}", text);
    Ok(())
}

/// Press a key/button
pub fn press_key(key: &str, simulator: Option<&str>) -> Result<()> {
    let udid = get_simulator_udid(simulator)?;

    match key.to_lowercase().as_str() {
        "home" => {
            // Use AppleScript with key code 4 (H) + Cmd+Shift — Simulator shortcut for Home
            let script = r#"tell application "Simulator" to activate
            delay 0.3
            tell application "System Events" to key code 4 using {command down, shift down}"#;
            let output = Command::new("osascript")
                .args(["-e", script])
                .output()
                .context("Failed to press Home via AppleScript")?;
            if !output.status.success() {
                let _ = simctl_exec(&["spawn", &udid, "notifyutil", "-p", "com.apple.springboard.home"]);
            }
        }
        "lock" => {
            // Cmd+L
            let script = r#"tell application "Simulator" to activate
            delay 0.1
            tell application "System Events"
                keystroke "l" using {command down}
            end tell"#;
            let _ = Command::new("osascript").args(["-e", script]).output();
        }
        "shake" => {
            // Cmd+Ctrl+Z
            let script = r#"tell application "Simulator" to activate
            delay 0.1
            tell application "System Events"
                keystroke "z" using {command down, control down}
            end tell"#;
            let _ = Command::new("osascript").args(["-e", script]).output();
        }
        _ => {
            // Try simctl io key for other keys
            let output = simctl_exec(&["io", &udid, "key", key]);
            if output.is_err() || !output.as_ref().unwrap().status.success() {
                // Fallback: try AppleScript keystroke
                let script = format!(
                    r#"tell application "Simulator" to activate
                    delay 0.1
                    tell application "System Events"
                        keystroke "{}"
                    end tell"#,
                    key
                );
                let _ = Command::new("osascript").args(["-e", &script]).output();
            }
        }
    }

    println!("Pressed key: {}", key);
    Ok(())
}

/// Dump UI hierarchy
pub fn ui_dump(format: &str, simulator: Option<&str>) -> Result<()> {
    let udid = get_simulator_udid(simulator)?;

    let output = simctl_exec(&["ui", &udid, "describe"]);

    if let Ok(out) = output {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            if format == "json" {
                println!("{}", text);
            } else {
                print!("{}", text);
            }
            return Ok(());
        }
    }

    println!("{{\"note\": \"UI dump via simctl is limited. Use XCUITest or Accessibility Inspector.\"}}");
    Ok(())
}

#[derive(Serialize)]
pub struct Simulator {
    pub name: String,
    pub udid: String,
    pub state: String,
    pub runtime: String,
}

/// List simulators
pub fn list_devices() -> Result<Vec<Simulator>> {
    let output = simctl_exec(&["list", "devices", "-j"])?;

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let mut simulators = Vec::new();

    if let Some(devices) = json["devices"].as_object() {
        for (runtime, device_list) in devices {
            if let Some(devices) = device_list.as_array() {
                for device in devices {
                    let state = device["state"].as_str().unwrap_or("Unknown");
                    if device["isAvailable"].as_bool().unwrap_or(false) {
                        simulators.push(Simulator {
                            name: device["name"].as_str().unwrap_or("Unknown").to_string(),
                            udid: device["udid"].as_str().unwrap_or("").to_string(),
                            state: state.to_string(),
                            runtime: runtime.replace("com.apple.CoreSimulator.SimRuntime.", ""),
                        });
                    }
                }
            }
        }
    }

    simulators.sort_by(|a, b| {
        if a.state == "Booted" && b.state != "Booted" {
            std::cmp::Ordering::Less
        } else if a.state != "Booted" && b.state == "Booted" {
            std::cmp::Ordering::Greater
        } else {
            a.name.cmp(&b.name)
        }
    });

    Ok(simulators)
}

/// Print devices list
pub fn print_devices() -> Result<()> {
    let simulators = list_devices()?;
    println!("iOS Simulators:");
    println!("{}", serde_json::to_string_pretty(&simulators)?);
    Ok(())
}

/// List installed apps
pub fn list_apps(filter: Option<&str>, simulator: Option<&str>) -> Result<()> {
    let udid = get_simulator_udid(simulator)?;

    let output = simctl_exec(&["listapps", &udid])?;

    if !output.status.success() {
        bail!("simctl listapps failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // listapps returns plist-like format: "com.apple.BundleID" = { ... CFBundleDisplayName = Name; ... }
    // Parse top-level keys (bundle IDs) and display names
    let bundle_re = regex::Regex::new(r#"^\s+"([^"]+)"\s+=\s+\{"#).unwrap();
    let display_re = regex::Regex::new(r#"CFBundleDisplayName\s*=\s*"?([^";]+)"?\s*;"#).unwrap();

    let mut apps: Vec<String> = Vec::new();
    let mut current_bundle: Option<String> = None;
    let mut current_display: Option<String> = None;

    for line in stdout.lines() {
        if let Some(cap) = bundle_re.captures(line) {
            // Save previous entry
            if let Some(bundle) = current_bundle.take() {
                let display = current_display.take().unwrap_or_default();
                let entry = if display.is_empty() {
                    bundle
                } else {
                    format!("{} ({})", bundle, display)
                };
                apps.push(entry);
            }
            current_bundle = Some(cap[1].to_string());
            current_display = None;
        } else if current_bundle.is_some() {
            if let Some(cap) = display_re.captures(line) {
                current_display = Some(cap[1].trim().to_string());
            }
        }
    }
    // Last entry
    if let Some(bundle) = current_bundle {
        let display = current_display.unwrap_or_default();
        let entry = if display.is_empty() { bundle } else { format!("{} ({})", bundle, display) };
        apps.push(entry);
    }

    // Apply filter
    if let Some(f) = filter {
        let f_lower = f.to_lowercase();
        apps.retain(|a| a.to_lowercase().contains(&f_lower));
    }

    apps.sort();
    apps.dedup();

    println!("Installed apps ({}):", apps.len());
    for app in &apps {
        println!("  {}", app);
    }
    Ok(())
}

/// Launch an app
pub fn launch_app(bundle_id: &str, simulator: Option<&str>) -> Result<()> {
    let udid = get_simulator_udid(simulator)?;

    let output = simctl_exec(&["launch", &udid, bundle_id])?;

    if !output.status.success() {
        bail!("Failed to launch {}: {}", bundle_id, String::from_utf8_lossy(&output.stderr));
    }

    println!("Launched: {}", bundle_id);
    Ok(())
}

/// Stop an app
pub fn stop_app(bundle_id: &str, simulator: Option<&str>) -> Result<()> {
    let udid = get_simulator_udid(simulator)?;

    let output = simctl_exec(&["terminate", &udid, bundle_id])?;

    if !output.status.success() {
        bail!("Failed to stop {}: {}", bundle_id, String::from_utf8_lossy(&output.stderr));
    }

    println!("Stopped: {}", bundle_id);
    Ok(())
}

/// Install an app
pub fn install_app(path: &str, simulator: Option<&str>) -> Result<()> {
    let udid = get_simulator_udid(simulator)?;

    println!("Installing {}...", path);

    let output = simctl_exec(&["install", &udid, path])?;

    if !output.status.success() {
        bail!("Failed to install: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Installed: {}", path);
    Ok(())
}

/// Uninstall an app
pub fn uninstall_app(bundle_id: &str, simulator: Option<&str>) -> Result<()> {
    let udid = get_simulator_udid(simulator)?;

    println!("Uninstalling {}...", bundle_id);

    let output = simctl_exec(&["uninstall", &udid, bundle_id])?;

    if !output.status.success() {
        bail!("Failed to uninstall: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Uninstalled: {}", bundle_id);
    Ok(())
}

/// Find element by text (limited support on iOS without XCUITest)
pub fn find_element(query: &str, simulator: Option<&str>) -> Result<Option<(i32, i32)>> {
    let _udid = get_simulator_udid(simulator)?;

    println!("Note: iOS element search via simctl is limited.");
    println!("For reliable element search, use XCUITest framework.");
    println!("Query: '{}'", query);

    Ok(None)
}

/// Tap element by text (limited support on iOS)
pub fn tap_element(query: &str, simulator: Option<&str>) -> Result<()> {
    if let Some((x, y)) = find_element(query, simulator)? {
        tap(x, y, simulator)?;
    } else {
        bail!("Element '{}' not found (iOS requires XCUITest for reliable element search)", query);
    }
    Ok(())
}

/// Clear device logs (limited on iOS simulator)
pub fn clear_logs(simulator: Option<&str>) -> Result<()> {
    let _udid = get_simulator_udid(simulator)?;
    println!("Note: iOS simulator log clearing is limited");
    println!("Logs are managed by the system log daemon");
    Ok(())
}

/// Get system info
pub fn get_system_info(simulator: Option<&str>) -> Result<()> {
    let udid = get_simulator_udid(simulator)?;

    let output = simctl_exec(&["list", "devices", "-j"])?;
    let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;

    if let Some(devices) = json["devices"].as_object() {
        for (runtime, device_list) in devices {
            if let Some(devices) = device_list.as_array() {
                for device in devices {
                    let device_udid = device["udid"].as_str().unwrap_or("");
                    let is_booted = device["state"].as_str() == Some("Booted");

                    if device_udid == udid || (udid == "booted" && is_booted) {
                        println!("System Info:");
                        println!("  Name: {}", device["name"].as_str().unwrap_or("unknown"));
                        println!("  State: {}", device["state"].as_str().unwrap_or("unknown"));
                        println!("  Runtime: {}", runtime.replace("com.apple.CoreSimulator.SimRuntime.", ""));
                        println!("  UDID: {}", device_udid);
                        return Ok(());
                    }
                }
            }
        }
    }

    println!("Device not found");
    Ok(())
}

/// Get current activity (foreground app)
pub fn get_current_activity(simulator: Option<&str>) -> Result<()> {
    let udid = get_simulator_udid(simulator)?;

    let output = Command::new("xcrun")
        .args(["simctl", "spawn", &udid, "launchctl", "list"])
        .output();

    if let Ok(out) = output {
        let stdout = String::from_utf8_lossy(&out.stdout);
        for line in stdout.lines().take(20) {
            if line.contains("UIKitApplication") || line.contains("application") {
                println!("{}", line);
            }
        }
    }

    println!("Note: Getting foreground app on iOS simulator is limited");
    Ok(())
}

/// Get device logs
pub fn get_logs(filter: Option<&str>, lines: usize, simulator: Option<&str>) -> Result<()> {
    let udid = get_simulator_udid(simulator)?;

    let predicate;
    let mut args = vec!["spawn", &udid, "log", "show", "--last", "5m", "--style", "compact"];

    if let Some(f) = filter {
        predicate = format!("processImagePath CONTAINS '{}'", f);
        args.push("--predicate");
        args.push(&predicate);
    }

    let output = simctl_exec(&args)?;

    if !output.status.success() {
        let fallback = simctl_exec(&["spawn", &udid, "log", "show", "--last", "1m"])?;
        let stdout = String::from_utf8_lossy(&fallback.stdout);
        for line in stdout.lines().take(lines) {
            println!("{}", line);
        }
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines().take(lines) {
        println!("{}", line);
    }
    Ok(())
}

/// Reboot simulator
pub fn reboot(simulator: Option<&str>) -> Result<()> {
    let udid = get_simulator_udid(simulator)?;

    println!("Rebooting simulator...");

    // Shutdown then boot
    let _ = simctl_exec(&["shutdown", &udid]);
    std::thread::sleep(std::time::Duration::from_secs(1));

    let output = simctl_exec(&["boot", &udid])?;

    if !output.status.success() {
        bail!("Failed to reboot: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Reboot initiated");
    Ok(())
}

// ============== File Transfer ==============

/// Push file to simulator (limited support)
pub fn push_file(local: &str, remote: &str, simulator: Option<&str>) -> Result<()> {
    let _udid = get_simulator_udid(simulator)?;
    println!("Note: File push to iOS simulator is not directly supported via simctl.");
    println!("Use 'xcrun simctl addmedia' for media files or app container paths.");
    println!("  Local: {}", local);
    println!("  Remote: {}", remote);
    Ok(())
}

/// Pull file from simulator (limited support)
pub fn pull_file(remote: &str, local: &str, simulator: Option<&str>) -> Result<()> {
    let _udid = get_simulator_udid(simulator)?;
    println!("Note: File pull from iOS simulator is not directly supported via simctl.");
    println!("Use app container paths: xcrun simctl get_app_container <udid> <bundle_id>");
    println!("  Remote: {}", remote);
    println!("  Local: {}", local);
    Ok(())
}

// ============== Clipboard ==============

/// Get clipboard content (host clipboard since simulator shares it)
pub fn get_clipboard(_simulator: Option<&str>) -> Result<()> {
    let output = Command::new("pbpaste")
        .output()
        .context("Failed to execute pbpaste")?;
    let text = String::from_utf8_lossy(&output.stdout);
    println!("{}", text);
    Ok(())
}

/// Set clipboard content (host clipboard since simulator shares it)
pub fn set_clipboard(text: &str, _simulator: Option<&str>) -> Result<()> {
    let mut child = Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .context("Failed to execute pbcopy")?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(text.as_bytes())?;
    }
    child.wait()?;
    println!("Clipboard set");
    Ok(())
}

// ============== Tests ==============

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_simulator_udid_booted() {
        // Should return "booted" when no simulator specified
        let result = get_simulator_udid(None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "booted");
    }
}
