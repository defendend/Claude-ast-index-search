//! Aurora OS device automation via audb

use std::process::Command;
use anyhow::{Result, Context, bail};
use serde::Serialize;

/// Build audb command with optional device serial
fn audb_cmd(device: Option<&str>) -> Command {
    let mut cmd = Command::new("audb");
    if let Some(serial) = device {
        cmd.arg("-s").arg(serial);
    }
    cmd
}

/// Execute audb command and return output
fn audb_exec(device: Option<&str>, args: &[&str]) -> Result<std::process::Output> {
    let mut cmd = audb_cmd(device);
    cmd.args(args);
    cmd.output().context("Failed to execute audb command")
}

/// Take screenshot and return PNG bytes
pub fn screenshot(device: Option<&str>) -> Result<Vec<u8>> {
    let output = audb_exec(device, &["exec-out", "screencap", "-p"])?;

    if !output.status.success() {
        bail!("audb screencap failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    Ok(output.stdout)
}

/// Tap at coordinates
pub fn tap(x: i32, y: i32, device: Option<&str>) -> Result<()> {
    let output = audb_exec(device, &["shell", "input", "tap", &x.to_string(), &y.to_string()])?;

    if !output.status.success() {
        bail!("audb tap failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Tapped at ({}, {})", x, y);
    Ok(())
}

/// Long press at coordinates
pub fn long_press(x: i32, y: i32, duration: u32, device: Option<&str>) -> Result<()> {
    let output = audb_exec(device, &[
        "shell", "input", "swipe",
        &x.to_string(), &y.to_string(),
        &x.to_string(), &y.to_string(),
        &duration.to_string(),
    ])?;

    if !output.status.success() {
        bail!("audb long press failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Long pressed at ({}, {}) for {}ms", x, y, duration);
    Ok(())
}

/// Swipe gesture
pub fn swipe(x1: i32, y1: i32, x2: i32, y2: i32, duration: u32, device: Option<&str>) -> Result<()> {
    let output = audb_exec(device, &[
        "shell", "input", "swipe",
        &x1.to_string(), &y1.to_string(),
        &x2.to_string(), &y2.to_string(),
        &duration.to_string(),
    ])?;

    if !output.status.success() {
        bail!("audb swipe failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Swiped from ({}, {}) to ({}, {})", x1, y1, x2, y2);
    Ok(())
}

/// Input text (with proper escaping)
pub fn input_text(text: &str, device: Option<&str>) -> Result<()> {
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

    let output = audb_exec(device, &["shell", "input", "text", &escaped])?;

    if !output.status.success() {
        bail!("audb input text failed: {}", String::from_utf8_lossy(&output.stderr));
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
        "space" => "KEYCODE_SPACE",
        "escape" | "esc" => "KEYCODE_ESCAPE",
        "up" => "KEYCODE_DPAD_UP",
        "down" => "KEYCODE_DPAD_DOWN",
        "left" => "KEYCODE_DPAD_LEFT",
        "right" => "KEYCODE_DPAD_RIGHT",
        _ => key,
    };

    let output = audb_exec(device, &["shell", "input", "keyevent", keycode])?;

    if !output.status.success() {
        bail!("audb keyevent failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Pressed key: {} ({})", key, keycode);
    Ok(())
}

/// Execute shell command on device
pub fn shell(command: &str, device: Option<&str>) -> Result<String> {
    let output = audb_exec(device, &["shell", command])?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() && !stderr.is_empty() {
        eprintln!("{}", stderr);
    }

    print!("{}", stdout);
    Ok(stdout)
}

/// Launch an app using Silica invoker
pub fn launch_app(package: &str, device: Option<&str>) -> Result<()> {
    let output = audb_exec(device, &[
        "shell", "invoker", "--type=silica-qt5", package,
    ])?;

    if !output.status.success() {
        bail!("Failed to launch {}: {}", package, String::from_utf8_lossy(&output.stderr));
    }

    println!("Launched: {}", package);
    Ok(())
}

/// Stop an app
pub fn stop_app(package: &str, device: Option<&str>) -> Result<()> {
    let output = audb_exec(device, &["shell", "pkill", "-f", package])?;

    if !output.status.success() {
        bail!("Failed to stop {}: {}", package, String::from_utf8_lossy(&output.stderr));
    }

    println!("Stopped: {}", package);
    Ok(())
}

/// Install an RPM package
pub fn install_app(path: &str, device: Option<&str>) -> Result<()> {
    println!("Installing {}...", path);

    let output = audb_exec(device, &["install", path])?;

    if !output.status.success() {
        bail!("Failed to install: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Installed: {}", path);
    Ok(())
}

/// Uninstall an app via rpm
pub fn uninstall_app(package: &str, device: Option<&str>) -> Result<()> {
    println!("Uninstalling {}...", package);

    let output = audb_exec(device, &["shell", "rpm", "-e", package])?;

    if !output.status.success() {
        bail!("Failed to uninstall: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Uninstalled: {}", package);
    Ok(())
}

/// Push file to device
pub fn push_file(local: &str, remote: &str, device: Option<&str>) -> Result<()> {
    let output = audb_exec(device, &["push", local, remote])?;

    if !output.status.success() {
        bail!("audb push failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Pushed {} -> {}", local, remote);
    Ok(())
}

/// Pull file from device
pub fn pull_file(remote: &str, local: &str, device: Option<&str>) -> Result<()> {
    let output = audb_exec(device, &["pull", remote, local])?;

    if !output.status.success() {
        bail!("audb pull failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Pulled {} -> {}", remote, local);
    Ok(())
}

/// Get device logs via journalctl
pub fn get_logs(filter: Option<&str>, lines: usize, device: Option<&str>) -> Result<()> {
    let lines_str = format!("{}", lines);
    let cmd = if let Some(f) = filter {
        format!("journalctl -n {} --grep={}", lines_str, f)
    } else {
        format!("journalctl -n {}", lines_str)
    };

    let output = audb_exec(device, &["shell", &cmd])?;

    if !output.status.success() {
        bail!("journalctl failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

/// Clear device logs
pub fn clear_logs(device: Option<&str>) -> Result<()> {
    let output = audb_exec(device, &[
        "shell", "journalctl --rotate && journalctl --vacuum-time=1s",
    ])?;

    if !output.status.success() {
        bail!("Failed to clear logs: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Logs cleared");
    Ok(())
}

/// Get system info (uname, os-release, memory)
pub fn get_system_info(device: Option<&str>) -> Result<()> {
    let uname = audb_exec(device, &["shell", "uname -a"])?;
    let uname_out = String::from_utf8_lossy(&uname.stdout);

    let os_release = audb_exec(device, &["shell", "cat /etc/os-release"])?;
    let os_release_out = String::from_utf8_lossy(&os_release.stdout);

    let mem = audb_exec(device, &["shell", "free -m"])?;
    let mem_out = String::from_utf8_lossy(&mem.stdout);

    println!("System Info:");
    println!("--- Kernel ---");
    print!("{}", uname_out);
    println!("--- OS Release ---");
    print!("{}", os_release_out);
    println!("--- Memory ---");
    print!("{}", mem_out);

    Ok(())
}

/// List installed apps (RPM packages)
pub fn list_apps(filter: Option<&str>, device: Option<&str>) -> Result<()> {
    let output = audb_exec(device, &["shell", "rpm -qa"])?;

    if !output.status.success() {
        bail!("rpm -qa failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut apps: Vec<&str> = stdout
        .lines()
        .filter(|line| {
            filter.map_or(true, |f| line.to_lowercase().contains(&f.to_lowercase()))
        })
        .collect();

    apps.sort();

    println!("Installed packages ({}):", apps.len());
    for app in &apps {
        println!("  {}", app);
    }
    Ok(())
}

/// Open URL via xdg-open
pub fn open_url(url: &str, device: Option<&str>) -> Result<()> {
    let output = audb_exec(device, &["shell", "xdg-open", url])?;

    if !output.status.success() {
        bail!("Failed to open URL: {}", String::from_utf8_lossy(&output.stderr));
    }

    println!("Opened URL: {}", url);
    Ok(())
}

// ============== Device Management ==============

#[derive(Serialize)]
pub struct Device {
    pub serial: String,
    pub state: String,
}

/// List connected devices
pub fn list_devices() -> Result<Vec<Device>> {
    let output = Command::new("audb")
        .arg("devices")
        .output()
        .context("Failed to execute audb devices")?;

    if !output.status.success() {
        bail!("audb devices failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut devices = Vec::new();

    for line in stdout.lines().skip(1) {
        if line.trim().is_empty() {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            devices.push(Device {
                serial: parts[0].to_string(),
                state: parts[1].to_string(),
            });
        }
    }

    Ok(devices)
}

/// Print devices list
pub fn print_devices() -> Result<()> {
    let devices = list_devices()?;
    println!("Aurora OS devices:");
    println!("{}", serde_json::to_string_pretty(&devices)?);
    Ok(())
}
