---
name: mobile-tools
description: "This skill should be used when the user asks to interact with device screens (screenshot, annotate, tap, swipe, type text), manage apps (install, launch, stop, uninstall), transfer files (push, pull), query device info (logs, system info, clipboard, screen size), run shell commands, manage desktop windows, or automate Android, iOS, Aurora OS, or Desktop apps."
---

# mobile-tools CLI

Fast CLI for mobile device automation across **Android** (via ADB), **iOS** (via simctl), **Aurora OS** (via audb), and **Desktop** (via companion JSON-RPC app).

Binary: `mobile-tools` (ensure it's in PATH or use full path to the built binary).

## Common Flags

Most commands accept platform-specific device selectors:

| Flag | Description | Platforms |
|------|-------------|-----------|
| `--device <serial>` | Android/Aurora device serial (default: first connected) | Android, Aurora |
| `--simulator <name>` | iOS Simulator name (default: booted) | iOS |
| `--companion-path <path>` | Path to Desktop companion app (or set `MOBILE_TOOLS_COMPANION` env) | Desktop |

---

## Commands Reference

### devices

List connected devices across platforms.

```bash
mobile-tools devices              # All platforms
mobile-tools devices android      # Android only
mobile-tools devices ios          # iOS simulators only
mobile-tools devices aurora       # Aurora devices only
```

**Platforms:** Android, iOS, Aurora

---

### screenshot

Capture a screenshot. Outputs base64 to stdout by default, or save to file with `-o`.

```bash
mobile-tools screenshot android
mobile-tools screenshot ios
mobile-tools screenshot aurora
mobile-tools screenshot desktop --companion-path /path/to/companion

# Save to file
mobile-tools screenshot android -o screen.png

# Compress for LLM (resize + JPEG quality reduction)
mobile-tools screenshot android --compress --max-width 800 --quality 60
```

| Flag | Description | Default |
|------|-------------|---------|
| `-o, --output <path>` | Save to file instead of base64 stdout | stdout |
| `-c, --compress` | Enable compression (resize + quality) | false |
| `--max-width <px>` | Max width when compressing | 1024 |
| `--max-height <px>` | Max height when compressing | unlimited |
| `--quality <1-100>` | JPEG quality when compressing | 80 |
| `--monitor-index <n>` | Monitor index (Desktop) | primary |

**Platforms:** Android, iOS, Aurora, Desktop

---

### annotate

Capture screenshot with UI element bounding boxes drawn over it. Useful for visual debugging and identifying tap targets.

```bash
mobile-tools annotate android -o annotated.png
mobile-tools annotate ios -o annotated.png
```

| Flag | Description |
|------|-------------|
| `-o, --output <path>` | Save to file instead of base64 stdout |

**Platforms:** Android, iOS

---

### analyze-screen

Parse current screen and return categorized interactive elements as structured JSON. Groups elements into buttons, inputs, texts, etc. Useful for automated test flows.

```bash
mobile-tools analyze-screen
mobile-tools analyze-screen --device emulator-5554
```

**Platforms:** Android only

---

### screen-size

Get screen resolution in pixels.

```bash
mobile-tools screen-size android
mobile-tools screen-size ios
```

**Platforms:** Android, iOS

---

### tap

Tap at exact coordinates, or by text/resource-id/index.

```bash
# By coordinates
mobile-tools tap android 500 800
mobile-tools tap ios 200 400
mobile-tools tap aurora 300 600
mobile-tools tap desktop 100 200 --companion-path /path/to/companion

# By text (searches UI tree, finds element, taps center)
mobile-tools tap android 0 0 --text "Login"
mobile-tools tap desktop 0 0 --text "Submit" --companion-path /path/to/companion

# By resource-id (Android)
mobile-tools tap android 0 0 --resource-id "btn_login"

# By element index from ui-dump (Android)
mobile-tools tap android 0 0 --index 5
```

| Flag | Description | Platforms |
|------|-------------|-----------|
| `--text <text>` | Tap element matching text | Android, Desktop |
| `--resource-id <id>` | Tap element by resource-id | Android |
| `--index <n>` | Tap element by ui-dump index | Android |

**Platforms:** Android, iOS, Aurora, Desktop

---

### tap-text

Find an element by text, resource-id, or content-desc in the UI hierarchy and tap it. Shortcut for `find` + `tap`.

```bash
mobile-tools tap-text android "Submit"
mobile-tools tap-text ios "Login"
```

**Platforms:** Android, iOS

---

### find

Search UI hierarchy for an element by text, resource-id, or content-desc. Returns element coordinates and bounds.

```bash
mobile-tools find android "Login"
mobile-tools find ios "Submit"
```

**Platforms:** Android, iOS

---

### find-and-tap

Fuzzy-match an element by description and tap it. Uses confidence scoring for inexact matches.

```bash
mobile-tools find-and-tap "Submit Order" --min-confidence 50
mobile-tools find-and-tap "Cancel" --min-confidence 30
```

| Flag | Description | Default |
|------|-------------|---------|
| `--min-confidence <0-100>` | Minimum match confidence threshold | 30 |

**Platforms:** Android only

---

### long-press

Long press at coordinates or by text. Duration configurable in milliseconds.

```bash
# By coordinates
mobile-tools long-press android 500 800 -d 2000
mobile-tools long-press ios 300 600
mobile-tools long-press aurora 400 700

# By text (Android: finds element, long presses at center)
mobile-tools long-press android 0 0 --text "Delete"
```

| Flag | Description | Default |
|------|-------------|---------|
| `-d, --duration <ms>` | Press duration in milliseconds | 1000 |
| `--text <text>` | Find by text and long press | — |

**Platforms:** Android, iOS, Aurora

---

### swipe

Swipe gesture between coordinates, or by named direction (up/down/left/right).

```bash
# By coordinates (x1 y1 x2 y2)
mobile-tools swipe android 500 1500 500 500 -d 300

# By direction (uses screen center, swipes 400px)
mobile-tools swipe android 0 0 0 0 --direction up
mobile-tools swipe ios 0 0 0 0 --direction left
mobile-tools swipe aurora 0 0 0 0 --direction down
```

| Flag | Description | Default |
|------|-------------|---------|
| `-d, --duration <ms>` | Swipe duration in milliseconds | 300 |
| `--direction <dir>` | Swipe direction: up, down, left, right (overrides coordinates) | — |

**Platforms:** Android, iOS, Aurora

---

### input

Type text into the currently focused field.

```bash
mobile-tools input android "Hello world"
mobile-tools input ios "Search query"
mobile-tools input aurora "user@example.com"
mobile-tools input desktop "text" --companion-path /path/to/companion
```

**Platforms:** Android, iOS, Aurora, Desktop

---

### key

Press a hardware/software key or button.

```bash
mobile-tools key android back
mobile-tools key android home
mobile-tools key android enter
mobile-tools key android power
mobile-tools key ios home
mobile-tools key aurora back
mobile-tools key desktop enter --companion-path /path/to/companion
```

Common keys: `home`, `back`, `enter`, `power`, `volume_up`, `volume_down`, `tab`, `delete`.

**Platforms:** Android, iOS, Aurora, Desktop

---

### ui-dump

Dump the current UI hierarchy. Default format is JSON; also supports XML for Android.

```bash
mobile-tools ui-dump android
mobile-tools ui-dump android -f xml
mobile-tools ui-dump ios
mobile-tools ui-dump desktop --companion-path /path/to/companion
```

| Flag | Description | Default |
|------|-------------|---------|
| `-f, --format <fmt>` | Output format: `json` or `xml` | json |
| `--show-all` | Include non-interactive elements (Android) | false |

**Platforms:** Android, iOS, Desktop

---

### apps

List installed applications, optionally filtered by name.

```bash
mobile-tools apps android
mobile-tools apps android -f "myapp"
mobile-tools apps ios
mobile-tools apps aurora
```

| Flag | Description |
|------|-------------|
| `-f, --filter <text>` | Filter by package/bundle name |

**Platforms:** Android, iOS, Aurora

---

### launch

Launch an application by package name, bundle ID, or path.

```bash
mobile-tools launch android com.example.app
mobile-tools launch ios com.example.app
mobile-tools launch aurora harbour-myapp
mobile-tools launch desktop /path/to/app --companion-path /path/to/companion
```

**Platforms:** Android, iOS, Aurora, Desktop

---

### stop

Force-stop/kill an application.

```bash
mobile-tools stop android com.example.app
mobile-tools stop ios com.example.app
mobile-tools stop aurora harbour-myapp
mobile-tools stop desktop "AppName" --companion-path /path/to/companion
```

**Platforms:** Android, iOS, Aurora, Desktop

---

### install

Install an application package onto the device.

```bash
mobile-tools install android /path/to/app.apk
mobile-tools install ios /path/to/app.app
mobile-tools install aurora /path/to/app.rpm
```

**Platforms:** Android, iOS, Aurora

---

### uninstall

Remove an installed application from the device.

```bash
mobile-tools uninstall android com.example.app
mobile-tools uninstall ios com.example.app
mobile-tools uninstall aurora harbour-myapp
```

**Platforms:** Android, iOS, Aurora

---

### push-file

Copy a local file to the device filesystem.

```bash
mobile-tools push-file android /local/path /sdcard/remote/path
mobile-tools push-file aurora /local/file /home/user/file
```

**Platforms:** Android, Aurora

---

### pull-file

Copy a file from device filesystem to local machine.

```bash
mobile-tools pull-file android /sdcard/remote/file /local/path
mobile-tools pull-file aurora /home/user/file /local/file
```

**Platforms:** Android, Aurora

---

### get-clipboard

Read current clipboard content from the device.

```bash
mobile-tools get-clipboard android
mobile-tools get-clipboard ios
mobile-tools get-clipboard desktop --companion-path /path/to/companion
```

**Platforms:** Android, iOS, Desktop

---

### set-clipboard

Set clipboard content on the device.

```bash
mobile-tools set-clipboard android "copied text"
mobile-tools set-clipboard ios "copied text"
mobile-tools set-clipboard desktop "text" --companion-path /path/to/companion
```

**Platforms:** Android, iOS, Desktop

---

### logs

Retrieve device logs. Supports line limit and filtering.

```bash
mobile-tools logs android -l 50
mobile-tools logs android -f "MyTag"
mobile-tools logs ios -l 200
mobile-tools logs aurora -l 100
```

| Flag | Description | Default |
|------|-------------|---------|
| `-l, --lines <n>` | Number of log lines to retrieve | 100 |
| `-f, --filter <text>` | Filter by tag/process/text | — |
| `--level <V/D/I/W/E/F>` | Log level filter (Android) | — |
| `--tag <tag>` | Filter by tag (Android) | — |
| `--package <pkg>` | Filter by package name (Android) | — |

**Platforms:** Android, iOS, Aurora

---

### clear-logs

Clear all device logs.

```bash
mobile-tools clear-logs android
mobile-tools clear-logs ios
mobile-tools clear-logs aurora
```

**Platforms:** Android, iOS, Aurora

---

### system-info

Get device system information (battery, memory, OS version, etc.).

```bash
mobile-tools system-info android
mobile-tools system-info ios
mobile-tools system-info aurora
```

**Platforms:** Android, iOS, Aurora

---

### current-activity

Get the currently displayed activity or foreground app.

```bash
mobile-tools current-activity android
mobile-tools current-activity ios
```

**Platforms:** Android, iOS

---

### reboot

Reboot the device or restart the simulator.

```bash
mobile-tools reboot android
mobile-tools reboot ios
```

**Platforms:** Android, iOS

---

### screen

Control screen power state (turn display on/off).

```bash
mobile-tools screen on
mobile-tools screen off
```

**Platforms:** Android only

---

### open-url

Open a URL in the device's default browser.

```bash
mobile-tools open-url android "https://example.com"
mobile-tools open-url ios "https://example.com"
mobile-tools open-url aurora "https://example.com"
```

**Platforms:** Android, iOS, Aurora

---

### shell

Execute an arbitrary shell command on the device.

```bash
mobile-tools shell android "ls /sdcard"
mobile-tools shell ios "ls ~/Documents"
mobile-tools shell aurora "uname -a"
```

**Platforms:** Android, iOS, Aurora

---

### wait

Pause execution for a specified duration. Useful in automation scripts between actions.

```bash
mobile-tools wait 2000    # wait 2 seconds
mobile-tools wait 500     # wait 500ms
```

**Platforms:** cross-platform (no device interaction)

---

### get-window-info

List all open desktop windows with their IDs, titles, positions, and sizes.

```bash
mobile-tools get-window-info --companion-path /path/to/companion
```

**Platforms:** Desktop only

---

### focus-window

Bring a desktop window to front by its ID (from `get-window-info`).

```bash
mobile-tools focus-window "window-id" --companion-path /path/to/companion
```

**Platforms:** Desktop only

---

### resize-window

Resize a desktop window to specified width and height.

```bash
mobile-tools resize-window "window-id" 800 600 --companion-path /path/to/companion
```

**Platforms:** Desktop only

---

### launch-desktop-app

Launch a desktop application by path.

```bash
mobile-tools launch-desktop-app /path/to/app --companion-path /path/to/companion
```

**Platforms:** Desktop only

---

### stop-desktop-app

Stop a running desktop application by name.

```bash
mobile-tools stop-desktop-app "AppName" --companion-path /path/to/companion
```

**Platforms:** Desktop only

---

### get-performance-metrics

Get CPU/memory usage metrics for running desktop applications.

```bash
mobile-tools get-performance-metrics --companion-path /path/to/companion
```

**Platforms:** Desktop only

---

### get-monitors

List connected monitors with resolutions and positions.

```bash
mobile-tools get-monitors --companion-path /path/to/companion
```

**Platforms:** Desktop only

---

## Additional Resources

For full platform support matrix and per-platform details (backends, supported/unsupported commands), see **`references/platform-support.md`**.

## Tips

- Use `--compress` on screenshots when sending to LLM — reduces token usage significantly
- `analyze-screen` gives structured JSON of buttons/inputs/texts — useful for automated testing
- `find-and-tap` uses fuzzy matching with confidence scoring — good for flaky element names
- Aurora commands use `audb` (Aurora Debug Bridge) — similar to ADB
- Desktop commands communicate via JSON-RPC with a companion app over stdin/stdout
- Combine `ui-dump` + `tap --index N` for reliable element interaction by index
- Use `wait` between actions in automation scripts to allow UI transitions
