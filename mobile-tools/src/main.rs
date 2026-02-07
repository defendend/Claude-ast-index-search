//! mobile-tools - Fast CLI for mobile device automation
//!
//! Supports Android (via ADB), iOS (via simctl), Aurora (via audb), Desktop (via companion app)

mod android;
mod aurora;
mod desktop;
mod ios;
mod screenshot;

use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "mobile-tools")]
#[command(about = "Fast CLI for mobile device automation (Android/iOS/Aurora/Desktop)")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Take a screenshot and optionally compress it
    Screenshot {
        /// Platform: android, ios, aurora, or desktop
        #[arg(value_parser = ["android", "ios", "aurora", "desktop"])]
        platform: String,

        /// Output file path (default: stdout as base64)
        #[arg(short, long)]
        output: Option<String>,

        /// Compress image (resize + quality reduction for LLM)
        #[arg(short, long, default_value = "false")]
        compress: bool,

        /// Max width for compression (default: 1024)
        #[arg(long, default_value = "1024")]
        max_width: u32,

        /// Max height for compression (default: unlimited)
        #[arg(long)]
        max_height: Option<u32>,

        /// JPEG quality for compression (1-100, default: 80)
        #[arg(long, default_value = "80")]
        quality: u8,

        /// iOS Simulator name (default: booted)
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial (default: first device)
        #[arg(long)]
        device: Option<String>,

        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,

        /// Monitor index for desktop screenshot
        #[arg(long)]
        monitor_index: Option<u32>,
    },

    /// Take annotated screenshot with UI element bounds
    Annotate {
        /// Platform: android or ios
        #[arg(value_parser = ["android", "ios"])]
        platform: String,

        /// Output file path (default: stdout as base64)
        #[arg(short, long)]
        output: Option<String>,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Tap at coordinates
    Tap {
        /// Platform: android, ios, aurora, or desktop
        #[arg(value_parser = ["android", "ios", "aurora", "desktop"])]
        platform: String,

        /// X coordinate
        x: i32,

        /// Y coordinate
        y: i32,

        /// Tap by text instead of coordinates (Android/Desktop)
        #[arg(long)]
        text: Option<String>,

        /// Tap by resource-id (Android)
        #[arg(long)]
        resource_id: Option<String>,

        /// Element index from ui-dump (Android)
        #[arg(long)]
        index: Option<usize>,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,

        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// Long press at coordinates
    LongPress {
        /// Platform: android, ios, or aurora
        #[arg(value_parser = ["android", "ios", "aurora"])]
        platform: String,

        /// X coordinate
        x: i32,

        /// Y coordinate
        y: i32,

        /// Duration in milliseconds (default: 1000)
        #[arg(short, long, default_value = "1000")]
        duration: u32,

        /// Long press by text (Android)
        #[arg(long)]
        text: Option<String>,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Open URL in browser
    OpenUrl {
        /// Platform: android, ios, or aurora
        #[arg(value_parser = ["android", "ios", "aurora"])]
        platform: String,

        /// URL to open
        url: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Execute shell command on device
    Shell {
        /// Platform: android, ios, or aurora
        #[arg(value_parser = ["android", "ios", "aurora"])]
        platform: String,

        /// Command to execute
        command: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Wait for specified duration
    Wait {
        /// Duration in milliseconds
        ms: u64,
    },

    /// Swipe gesture
    Swipe {
        /// Platform: android, ios, or aurora
        #[arg(value_parser = ["android", "ios", "aurora"])]
        platform: String,

        /// Start X
        x1: i32,

        /// Start Y
        y1: i32,

        /// End X
        x2: i32,

        /// End Y
        y2: i32,

        /// Duration in milliseconds (default: 300)
        #[arg(short, long, default_value = "300")]
        duration: u32,

        /// Swipe direction (up/down/left/right) - overrides coordinates
        #[arg(long)]
        direction: Option<String>,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Input text
    Input {
        /// Platform: android, ios, aurora, or desktop
        #[arg(value_parser = ["android", "ios", "aurora", "desktop"])]
        platform: String,

        /// Text to input
        text: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,

        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// Press a key/button
    Key {
        /// Platform: android, ios, aurora, or desktop
        #[arg(value_parser = ["android", "ios", "aurora", "desktop"])]
        platform: String,

        /// Key name (home, back, enter, etc.)
        key: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,

        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// Dump UI hierarchy
    UiDump {
        /// Platform: android, ios, or desktop
        #[arg(value_parser = ["android", "ios", "desktop"])]
        platform: String,

        /// Output format: json or xml
        #[arg(short, long, default_value = "json")]
        format: String,

        /// Show all elements including non-interactive (Android)
        #[arg(long, default_value = "false")]
        show_all: bool,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android device serial
        #[arg(long)]
        device: Option<String>,

        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// List connected devices
    Devices {
        /// Platform: android, ios, aurora, or all
        #[arg(value_parser = ["android", "ios", "aurora", "all"], default_value = "all")]
        platform: String,
    },

    /// List installed apps
    Apps {
        /// Platform: android, ios, or aurora
        #[arg(value_parser = ["android", "ios", "aurora"])]
        platform: String,

        /// Filter by package/bundle name
        #[arg(short, long)]
        filter: Option<String>,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Launch an app
    Launch {
        /// Platform: android, ios, aurora, or desktop
        #[arg(value_parser = ["android", "ios", "aurora", "desktop"])]
        platform: String,

        /// Package name (Android/Aurora) or bundle ID (iOS) or app path (Desktop)
        package: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,

        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// Stop/kill an app
    Stop {
        /// Platform: android, ios, aurora, or desktop
        #[arg(value_parser = ["android", "ios", "aurora", "desktop"])]
        platform: String,

        /// Package name (Android/Aurora) or bundle ID (iOS) or app name (Desktop)
        package: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,

        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// Uninstall an app
    Uninstall {
        /// Platform: android, ios, or aurora
        #[arg(value_parser = ["android", "ios", "aurora"])]
        platform: String,

        /// Package name (Android/Aurora) or bundle ID (iOS)
        package: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Install an app
    Install {
        /// Platform: android, ios, or aurora
        #[arg(value_parser = ["android", "ios", "aurora"])]
        platform: String,

        /// Path to APK (Android), app bundle (iOS), or RPM (Aurora)
        path: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Find element by text/resource-id and get coordinates
    Find {
        /// Platform: android or ios
        #[arg(value_parser = ["android", "ios"])]
        platform: String,

        /// Text, resource-id, or content-desc to search for
        query: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Tap element by text/resource-id
    TapText {
        /// Platform: android or ios
        #[arg(value_parser = ["android", "ios"])]
        platform: String,

        /// Text, resource-id, or content-desc to tap
        query: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Get device logs
    Logs {
        /// Platform: android, ios, or aurora
        #[arg(value_parser = ["android", "ios", "aurora"])]
        platform: String,

        /// Filter by tag/process
        #[arg(short, long)]
        filter: Option<String>,

        /// Number of lines (default: 100)
        #[arg(short, long, default_value = "100")]
        lines: usize,

        /// Log level filter (Android: V/D/I/W/E/F)
        #[arg(long)]
        level: Option<String>,

        /// Filter by tag (Android)
        #[arg(long)]
        tag: Option<String>,

        /// Filter by package name (Android)
        #[arg(long)]
        package: Option<String>,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Clear device logs
    ClearLogs {
        /// Platform: android, ios, or aurora
        #[arg(value_parser = ["android", "ios", "aurora"])]
        platform: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Get system info (battery, memory)
    SystemInfo {
        /// Platform: android, ios, or aurora
        #[arg(value_parser = ["android", "ios", "aurora"])]
        platform: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android/Aurora device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Get current activity/foreground app
    CurrentActivity {
        /// Platform: android or ios
        #[arg(value_parser = ["android", "ios"])]
        platform: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Reboot device/simulator
    Reboot {
        /// Platform: android or ios
        #[arg(value_parser = ["android", "ios"])]
        platform: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Control screen power (Android only)
    Screen {
        /// Turn screen on or off
        #[arg(value_parser = ["on", "off"])]
        state: String,

        /// Android device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Get screen resolution
    ScreenSize {
        /// Platform: android or ios
        #[arg(value_parser = ["android", "ios"])]
        platform: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android device serial
        #[arg(long)]
        device: Option<String>,
    },

    // ===== New commands =====

    /// Analyze screen structure (Android only)
    AnalyzeScreen {
        /// Android device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Find element by fuzzy description and tap it (Android only)
    FindAndTap {
        /// Description to match
        description: String,

        /// Minimum confidence threshold (0-100, default: 30)
        #[arg(long, default_value = "30")]
        min_confidence: u32,

        /// Android device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Push file to device
    PushFile {
        /// Platform: android or aurora
        #[arg(value_parser = ["android", "aurora"])]
        platform: String,

        /// Local file path
        local: String,

        /// Remote file path on device
        remote: String,

        /// Device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Pull file from device
    PullFile {
        /// Platform: android or aurora
        #[arg(value_parser = ["android", "aurora"])]
        platform: String,

        /// Remote file path on device
        remote: String,

        /// Local file path
        local: String,

        /// Device serial
        #[arg(long)]
        device: Option<String>,
    },

    /// Get clipboard content
    GetClipboard {
        /// Platform: android, ios, or desktop
        #[arg(value_parser = ["android", "ios", "desktop"])]
        platform: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android device serial
        #[arg(long)]
        device: Option<String>,

        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// Set clipboard content
    SetClipboard {
        /// Platform: android, ios, or desktop
        #[arg(value_parser = ["android", "ios", "desktop"])]
        platform: String,

        /// Text to set
        text: String,

        /// iOS Simulator name
        #[arg(long)]
        simulator: Option<String>,

        /// Android device serial
        #[arg(long)]
        device: Option<String>,

        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// Get performance metrics (Desktop only)
    GetPerformanceMetrics {
        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// List monitors (Desktop only)
    GetMonitors {
        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// Launch desktop app
    LaunchDesktopApp {
        /// App path
        app_path: String,

        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// Stop desktop app
    StopDesktopApp {
        /// App name
        app_name: String,

        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// Get desktop window info
    GetWindowInfo {
        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// Focus a desktop window
    FocusWindow {
        /// Window ID
        window_id: String,

        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },

    /// Resize a desktop window
    ResizeWindow {
        /// Window ID
        window_id: String,

        /// Width
        width: u32,

        /// Height
        height: u32,

        /// Desktop companion app path
        #[arg(long)]
        companion_path: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    let result = run(cli);

    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {}", e);
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Screenshot {
            platform,
            output,
            compress,
            max_width,
            max_height: _,
            quality,
            simulator,
            device,
            companion_path,
            monitor_index: _,
        } => {
            if platform == "desktop" {
                let data = desktop::screenshot(companion_path.as_deref())?;
                if let Some(path) = output.as_deref() {
                    std::fs::write(path, &data)?;
                    eprintln!("Screenshot saved to: {}", path);
                } else {
                    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
                    println!("{}", b64);
                }
                return Ok(());
            }
            if platform == "aurora" {
                let data = aurora::screenshot(device.as_deref())?;
                if let Some(path) = output.as_deref() {
                    std::fs::write(path, &data)?;
                    eprintln!("Screenshot saved to: {}", path);
                } else {
                    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &data);
                    println!("{}", b64);
                }
                return Ok(());
            }
            screenshot::take_screenshot(
                &platform,
                output.as_deref(),
                compress,
                max_width,
                quality,
                simulator.as_deref(),
                device.as_deref(),
            )
        }

        Commands::Annotate {
            platform,
            output,
            simulator,
            device,
        } => {
            screenshot::take_annotated_screenshot(
                &platform,
                output.as_deref(),
                device.as_deref(),
                simulator.as_deref(),
            )
        }

        Commands::Tap {
            platform,
            x,
            y,
            text,
            resource_id: _,
            index: _,
            simulator,
            device,
            companion_path,
        } => {
            if let Some(t) = text {
                if platform == "desktop" {
                    return desktop::tap_by_text(&t, companion_path.as_deref());
                }
                return android::tap_element(&t, device.as_deref());
            }
            match platform.as_str() {
                "android" => android::tap(x, y, device.as_deref()),
                "ios" => ios::tap(x, y, simulator.as_deref()),
                "aurora" => aurora::tap(x, y, device.as_deref()),
                "desktop" => desktop::tap(x, y, companion_path.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::Swipe {
            platform,
            mut x1,
            mut y1,
            mut x2,
            mut y2,
            duration,
            direction,
            simulator,
            device,
        } => {
            // If direction is provided, compute coordinates from screen center
            if let Some(dir) = direction {
                // Use reasonable defaults for swipe from center
                let (cx, cy) = (540, 960);
                let dist = 400;
                match dir.to_lowercase().as_str() {
                    "up" => { x1 = cx; y1 = cy + dist; x2 = cx; y2 = cy - dist; }
                    "down" => { x1 = cx; y1 = cy - dist; x2 = cx; y2 = cy + dist; }
                    "left" => { x1 = cx + dist; y1 = cy; x2 = cx - dist; y2 = cy; }
                    "right" => { x1 = cx - dist; y1 = cy; x2 = cx + dist; y2 = cy; }
                    _ => {}
                }
            }
            match platform.as_str() {
                "android" => android::swipe(x1, y1, x2, y2, duration, device.as_deref()),
                "ios" => ios::swipe(x1, y1, x2, y2, duration, simulator.as_deref()),
                "aurora" => aurora::swipe(x1, y1, x2, y2, duration, device.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::Input {
            platform,
            text,
            simulator,
            device,
            companion_path,
        } => {
            match platform.as_str() {
                "android" => android::input_text(&text, device.as_deref()),
                "ios" => ios::input_text(&text, simulator.as_deref()),
                "aurora" => aurora::input_text(&text, device.as_deref()),
                "desktop" => desktop::input_text(&text, companion_path.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::Key {
            platform,
            key,
            simulator,
            device,
            companion_path,
        } => {
            match platform.as_str() {
                "android" => android::press_key(&key, device.as_deref()),
                "ios" => ios::press_key(&key, simulator.as_deref()),
                "aurora" => aurora::press_key(&key, device.as_deref()),
                "desktop" => desktop::press_key(&key, companion_path.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::UiDump {
            platform,
            format,
            show_all: _,
            simulator,
            device,
            companion_path,
        } => {
            match platform.as_str() {
                "android" => android::ui_dump(&format, device.as_deref()),
                "ios" => ios::ui_dump(&format, simulator.as_deref()),
                "desktop" => desktop::get_ui(companion_path.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::Devices { platform } => {
            match platform.as_str() {
                "android" => android::print_devices(),
                "ios" => ios::print_devices(),
                "aurora" => aurora::print_devices(),
                _ => {
                    android::print_devices()?;
                    ios::print_devices()?;
                    aurora::print_devices()
                }
            }
        }

        Commands::Apps {
            platform,
            filter,
            simulator,
            device,
        } => {
            match platform.as_str() {
                "android" => android::list_apps(filter.as_deref(), device.as_deref()),
                "ios" => ios::list_apps(filter.as_deref(), simulator.as_deref()),
                "aurora" => aurora::list_apps(filter.as_deref(), device.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::Launch {
            platform,
            package,
            simulator,
            device,
            companion_path,
        } => {
            match platform.as_str() {
                "android" => android::launch_app(&package, device.as_deref()),
                "ios" => ios::launch_app(&package, simulator.as_deref()),
                "aurora" => aurora::launch_app(&package, device.as_deref()),
                "desktop" => desktop::launch_app(&package, companion_path.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::Stop {
            platform,
            package,
            simulator,
            device,
            companion_path,
        } => {
            match platform.as_str() {
                "android" => android::stop_app(&package, device.as_deref()),
                "ios" => ios::stop_app(&package, simulator.as_deref()),
                "aurora" => aurora::stop_app(&package, device.as_deref()),
                "desktop" => desktop::stop_app(&package, companion_path.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::Install {
            platform,
            path,
            simulator,
            device,
        } => {
            match platform.as_str() {
                "android" => android::install_app(&path, device.as_deref()),
                "ios" => ios::install_app(&path, simulator.as_deref()),
                "aurora" => aurora::install_app(&path, device.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::Find {
            platform,
            query,
            simulator,
            device,
        } => {
            if platform == "android" {
                android::find_element(&query, device.as_deref())?;
            } else {
                ios::find_element(&query, simulator.as_deref())?;
            }
            Ok(())
        }

        Commands::TapText {
            platform,
            query,
            simulator,
            device,
        } => {
            if platform == "android" {
                android::tap_element(&query, device.as_deref())
            } else {
                ios::tap_element(&query, simulator.as_deref())
            }
        }

        Commands::Logs {
            platform,
            filter,
            lines,
            level: _,
            tag: _,
            package: _,
            simulator,
            device,
        } => {
            match platform.as_str() {
                "android" => android::get_logs(filter.as_deref(), lines, device.as_deref()),
                "ios" => ios::get_logs(filter.as_deref(), lines, simulator.as_deref()),
                "aurora" => aurora::get_logs(filter.as_deref(), lines, device.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::LongPress {
            platform,
            x,
            y,
            duration,
            text,
            simulator,
            device,
        } => {
            if let Some(t) = text {
                // Find by text then long press at center
                if let Some((cx, cy)) = android::find_element(&t, device.as_deref())? {
                    return android::long_press(cx, cy, duration, device.as_deref());
                }
                anyhow::bail!("Element '{}' not found for long press", t);
            }
            match platform.as_str() {
                "android" => android::long_press(x, y, duration, device.as_deref()),
                "ios" => ios::long_press(x, y, duration, simulator.as_deref()),
                "aurora" => aurora::long_press(x, y, duration, device.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::OpenUrl {
            platform,
            url,
            simulator,
            device,
        } => {
            match platform.as_str() {
                "android" => android::open_url(&url, device.as_deref()),
                "ios" => ios::open_url(&url, simulator.as_deref()),
                "aurora" => aurora::open_url(&url, device.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::Shell {
            platform,
            command,
            simulator,
            device,
        } => {
            match platform.as_str() {
                "android" => { android::shell(&command, device.as_deref())?; }
                "ios" => { ios::shell(&command, simulator.as_deref())?; }
                "aurora" => { aurora::shell(&command, device.as_deref())?; }
                _ => unreachable!(),
            }
            Ok(())
        }

        Commands::Wait { ms } => {
            std::thread::sleep(std::time::Duration::from_millis(ms));
            println!("Waited {}ms", ms);
            Ok(())
        }

        Commands::ClearLogs {
            platform,
            simulator,
            device,
        } => {
            match platform.as_str() {
                "android" => android::clear_logs(device.as_deref()),
                "ios" => ios::clear_logs(simulator.as_deref()),
                "aurora" => aurora::clear_logs(device.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::SystemInfo {
            platform,
            simulator,
            device,
        } => {
            match platform.as_str() {
                "android" => android::get_system_info(device.as_deref()),
                "ios" => ios::get_system_info(simulator.as_deref()),
                "aurora" => aurora::get_system_info(device.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::CurrentActivity {
            platform,
            simulator,
            device,
        } => {
            if platform == "android" {
                android::get_current_activity(device.as_deref())
            } else {
                ios::get_current_activity(simulator.as_deref())
            }
        }

        Commands::Uninstall {
            platform,
            package,
            simulator,
            device,
        } => {
            match platform.as_str() {
                "android" => android::uninstall_app(&package, device.as_deref()),
                "ios" => ios::uninstall_app(&package, simulator.as_deref()),
                "aurora" => aurora::uninstall_app(&package, device.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::Reboot {
            platform,
            simulator,
            device,
        } => {
            if platform == "android" {
                android::reboot(device.as_deref())
            } else {
                ios::reboot(simulator.as_deref())
            }
        }

        Commands::Screen { state, device } => {
            let on = state == "on";
            android::screen_power(on, device.as_deref())
        }

        Commands::ScreenSize {
            platform,
            simulator,
            device,
        } => {
            if platform == "android" {
                let (w, h) = android::get_screen_size(device.as_deref())?;
                println!("Screen size: {}x{}", w, h);
                Ok(())
            } else {
                // Get screen size from screenshot dimensions
                let data = ios::screenshot(simulator.as_deref())?;
                let img = image::load_from_memory(&data)?;
                println!("Screen size: {}x{}", img.width(), img.height());
                Ok(())
            }
        }

        // ===== New commands =====

        Commands::AnalyzeScreen { device } => {
            android::analyze_screen(device.as_deref())
        }

        Commands::FindAndTap {
            description,
            min_confidence,
            device,
        } => {
            android::find_and_tap(&description, min_confidence, device.as_deref())
        }

        Commands::PushFile {
            platform,
            local,
            remote,
            device,
        } => {
            match platform.as_str() {
                "android" => android::push_file(&local, &remote, device.as_deref()),
                "aurora" => aurora::push_file(&local, &remote, device.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::PullFile {
            platform,
            remote,
            local,
            device,
        } => {
            match platform.as_str() {
                "android" => android::pull_file(&remote, &local, device.as_deref()),
                "aurora" => aurora::pull_file(&remote, &local, device.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::GetClipboard {
            platform,
            simulator: _,
            device,
            companion_path,
        } => {
            match platform.as_str() {
                "android" => android::get_clipboard(device.as_deref()),
                "ios" => ios::get_clipboard(None),
                "desktop" => desktop::get_clipboard(companion_path.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::SetClipboard {
            platform,
            text,
            simulator: _,
            device,
            companion_path,
        } => {
            match platform.as_str() {
                "android" => android::set_clipboard(&text, device.as_deref()),
                "ios" => ios::set_clipboard(&text, None),
                "desktop" => desktop::set_clipboard(&text, companion_path.as_deref()),
                _ => unreachable!(),
            }
        }

        Commands::GetPerformanceMetrics { companion_path } => {
            desktop::get_performance_metrics(companion_path.as_deref())
        }

        Commands::GetMonitors { companion_path } => {
            desktop::get_monitors(companion_path.as_deref())
        }

        Commands::LaunchDesktopApp { app_path, companion_path } => {
            desktop::launch_app(&app_path, companion_path.as_deref())
        }

        Commands::StopDesktopApp { app_name, companion_path } => {
            desktop::stop_app(&app_name, companion_path.as_deref())
        }

        Commands::GetWindowInfo { companion_path } => {
            desktop::get_window_info(companion_path.as_deref())
        }

        Commands::FocusWindow { window_id, companion_path } => {
            desktop::focus_window(&window_id, companion_path.as_deref())
        }

        Commands::ResizeWindow { window_id, width, height, companion_path } => {
            desktop::resize_window(&window_id, width, height, companion_path.as_deref())
        }
    }
}
