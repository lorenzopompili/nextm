<p align="center">
  <img src="logo/PNG/hal_ecg_128.png" alt="nextm logo" width="128" height="128" />
</p>

<h1 align="center">nextm</h1>

<p align="center">
  <strong>Lightweight System Monitor, Compact Task Manager, Windows Services Manager & Network Connections Inspector (TCPView Slim) for Windows 11</strong><br />
  <em>Crafted with precision by <strong>Lorenzo Pompili</strong></em>
</p>

<p align="center">
  <a href="#-why-nextm-was-created"><img src="https://img.shields.io/badge/Origin-Personal%20Vision-blue.svg" alt="Origin" /></a>
  <img src="https://img.shields.io/badge/Language-Rust%202024-orange.svg" alt="Rust 2024" />
  <img src="https://img.shields.io/badge/Platform-Windows%2011%20(x64%20%7C%20ARM64)-0078D4.svg" alt="Windows 11" />
  <img src="https://img.shields.io/badge/GUI-Pure%20Win32%20GDI-success.svg" alt="Pure Win32 GDI" />
  <img src="https://img.shields.io/badge/Runtime%20DLLs-0%20External-brightgreen.svg" alt="Zero External DLLs" />
  <img src="https://img.shields.io/badge/License-MIT%20%2F%20Apache--2.0-lightgrey.svg" alt="License" />
</p>

---

## 💡 Why nextm was created

> *"I needed a monitor and task manager that was instant to open and use, consumed negligible system resources, and brought together everything I need on a daily basis in a single place: unobtrusive system tray monitoring, quick process management, Windows service control, and a slim, immediate version of TCPView."*  
> — **Lorenzo Pompili**, creator of nextm

On modern Windows, essential system diagnostic utilities have become bloated and fragmented:
- **Windows Task Manager** has transitioned to modern XAML/WinUI frameworks, consuming hundreds of megabytes of RAM, taking seconds to open, and incurring noticeable background CPU overhead.
- Managing or restarting a background Windows service requires opening the sluggish and dated Microsoft Management Console (`services.msc`).
- Diagnosing active network sockets, listening ports, or rogue outbound connections requires launching heavy external utilities like Sysinternals TCPView.
- Third-party system monitors often install intrusive ring-0 kernel drivers, demand permanent Administrator privileges, harvest telemetry, or consume more resources than the system components they monitor.

**nextm was built to solve all of this in a single ~1 MB self-contained binary:**
1. **Instantaneous:** Launches in under 95 milliseconds, idling at less than 3 MB of RAM and ~0.6% of a single core.
2. **All-in-One Powerhouse:** Combines Process Management (hierarchical tree), Network Connections (TCP/UDP), Windows Services, System Telemetry, and Settings in a unified, flicker-free window.
3. **Zero Ring-0 / Zero Kernel Drivers:** Reads CPU/ACPI thermal zones, GPU metrics (D3DKMT), and NVMe/SSD telemetry using official Windows/NT APIs without deploying third-party kernel drivers.
4. **Zero External Runtime Dependencies:** Written in 100% pure Rust with direct Win32/NT system APIs and double-buffered GDI. No .NET, WebView2, Electron, or Visual C++ Redistributable required.
5. **Native Bilingual Interface (English & Italian):** Automatically aligns with the Windows display language out of the box, with instant live switching available in Settings.

---

## ✨ Core Features

### 1. Unobtrusive System Tray Monitoring
- **Independent or Combined Metrics:** Display dedicated, real-time tray icons for:
  - **CPU:** Global utilization (Standard occupancy or PDH Utility scaled to nominal/turbo clock speeds) with per-core saturation detection (subtle amber 1–4 segment indicator bar) and active **Thermal/Power Throttling detection** (`⚠️ THERMAL / POWER THROTTLING ACTIVE`) via `CallNtPowerInformation`.
  - **RAM:** Physical memory percentage, used GiB, and available GiB.
  - **Network:** Real-time download and upload speeds with dynamic auto-scaling (B/s, KB/s, MB/s, GB/s, or bit/s).
  - **Disk I/O Throughput:** Real-time Read and Write speeds (MB/s or KB/s) for all connected drives (internal NVMe/SATA SSDs and external USB drives) via `IOCTL_DISK_PERFORMANCE`.
  - **Hardware Temperatures:** ACPI/CPU thermal zones, discrete/integrated GPU, and NVMe SSD / storage drives.
  - **Dedicated GPU Usage %:** Real-time GPU load percentage with dedicated graphics card pixel-art glyph and tiered alert coloring.
- **Customizable Icon Styles:**
  - **Numbers Only:** High-definition tabular pixel-art digits specifically rasterized for 16, 20, 24, 28, and 32 px tray sizes.
  - **Icon + Number:** Thematic pixel-art glyph (microchip, DIMM, thermometer, graphics card, drive) with values underneath.
- **Intelligent Contrast & Theme Awareness:** Dynamically adapts foreground contrast to light/dark Windows taskbars, transitions to amber at 70% and red at 90% (with anti-jitter hysteresis), and provides full Windows High Contrast Mode support.
- **Rich, Flicker-Free Hover Summary Panel:** Hovering over any tray icon renders a clean, anchored floating card (without stealing focus from your active application) detailing all hardware resources, per-core logical loads, disk throughput, thermal throttling state, and a customizable list of top resource-consuming processes (from 1 to 25).

---

### 2. Unified Inspection Window ("The Big Step")
A single **left-click** or **double-click** on any nextm tray icon brings up the main inspection window, styled with Windows 11 DWM rounded corners, native dark/light mode, and zero flicker:

```
┌────────────────────────────────────────────────────────────────────────────────────────┐
│   [ Processes ]   [ Connections ]   [ Services ]   [ Settings ]   [ About ]            │
└────────────────────────────────────────────────────────────────────────────────────────┘
```

#### 🔹 Processes Tab (Snappy Task Manager)
- High-frequency kernel scanning via `NtQuerySystemInformation(SystemProcessInformation)`.
- **Hierarchical Tree View:** Groups multiple instances of the same executable (e.g. web browsers, Discord, background daemons) with instant expand/collapse via arrow keys, Spacebar, or mouse click.
- **In-Depth Metrics:** CPU %, Working Set (physical RAM), Private Bytes (commit charge), active threads, open network sockets, and hosted Windows services count.
- **Quick Kill for Hung Windows:** Continuously monitors UI responsiveness via `IsHungAppWindow`. Instantly renders a high-visibility header banner with one-click `[Kill now]` action and flags the frozen application in the list with `[⚠️ Not Responding]`.
- **Memory Leak Hunter:** Continuously inspects private memory trends, flagging persistent, monotonic memory bloat ($\ge 15\text{ MB}$ without release) with `[⚠️ Leak +X MB]`. Instantly filterable in the search bar via `leak:` or `leak`.
- **Real-Time Instant Search:** Zero-allocation ASCII search filter for instant matching against process names, PIDs, hosted services, or memory leaks.
- **Safe Forced Termination:** End unresponsive tasks directly from the grid with the `Del` key or the action button.

#### 🔹 Network Connections Tab (Slim TCPView)
- Direct inspection of extended TCP and UDP tables (both IPv4 and IPv6) via `iphlpapi.dll`.
- **$O(1)$ Process Correlation:** Automatically maps local and remote ports and endpoints to their parent process name and PID.
- **Geo-IP Scope Classification:** Categorizes every remote endpoint into `[LAN]` (private RFC 1918), `[WAN]` (public internet), `[Loopback]` (localhost), or `[CGNAT]`.
- **One-Click WHOIS & Geolocation:** Context menu (right-click) on WAN sockets to query `ipinfo.io` (IP address, ISP, AS number, country/city) directly in your default web browser.
- Displays all TCP states (`Established`, `Listen`, `TimeWait`, `CloseWait`, etc.).
- Groups sockets by application with a fast action to **terminate/close active TCP connections**.

#### 🔹 Windows Services Tab
- Native interface to the Windows Service Control Manager (`advapi32.dll`).
- Dissects composite host processes (e.g. reveals exactly which service runs inside which `svchost.exe` instance).
- Real-time service states (Running, Stopped, Paused, etc.).
- One-click controls to **Start** or **Stop** any Windows service.

#### 🔹 Built-in Settings Tab
Cleanly structured into customizable visual cards:
1. **Metrics & Icons:** Toggle tray icons individually or in combination (CPU, RAM, Network, Disk I/O, ACPI, GPU Temp, GPU Usage %), select icon visual presentation (Numbers Only vs Icon + Number), choose network measurement units (Bytes/s vs Bits/s), and select interface language (**Auto OS**, **Italiano**, or **English**).
2. **Computation & Performance:** Choose between Standard CPU and PDH Utility modes, toggle per-core hover breakdown, adjust telemetry polling frequency (1s, 2s, 5s), activate Windows EcoQoS efficiency, and configure the number of Top offending processes displayed on hover (free-range 1 to 25).
3. **Proactive Alerts & Critical Thresholds:** Set custom critical alert percentages (1% to 100%) for CPU, RAM, and thermal alarm triggers.
4. **Blackbox Flight Recorder:** Enable or disable continuous telemetry logging to `%LOCALAPPDATA%\nextm\blackbox\nextm_blackbox.csv` with automatic 5 MB rotation for post-crash diagnosis and hardware troubleshooting.
5. **Startup & Privileges:**
   - **Start with Windows:** Standard user auto-start registry key (`Run`).
   - **Start with Highest Privileges (Admin Task):** Creates a Windows Scheduled Task (`schtasks /rl highest`) to launch nextm elevated at login **without triggering a UAC prompt**.
   - **"🛡️ Restart as Administrator" Button:** On-demand privilege elevation with seamless single-instance handoff.
   - Storage mode indicator (Windows Registry vs portable `nextm.ini`).

#### 🔹 About Tab
- High-definition branding, build version, author attribution to **Lorenzo Pompili**, and live security token state (Elevated Administrator with `SeDebugPrivilege` vs Standard User).

---

## 🚀 Installation & Getting Started

nextm is distributed in two formats:

### Option A: Modern Windows Installer (Recommended)
Download `nextm-setup-v0.1.4.exe`:
- **Bilingual Setup:** Select between **English** and **Italiano** at launch.
- **No UAC Required:** Installs cleanly to `%LOCALAPPDATA%\Programs\nextm`.
- Creates Start Menu and optional Desktop shortcuts.
- Configures automatic startup at login.
- **100% Clean Uninstaller:** Integrated into *Windows Settings › Installed Apps*. Safely terminates running instances, deletes scheduled tasks, cleans application binaries, and purges all user registry keys (`HKCU\Software\nextm`).

### Option B: Self-Contained Portable Edition
Download `nextm.exe` or `nextm-v0.1.4-windows-x64-portable.zip`:
- A single, self-contained binary with zero installers and zero prerequisites.
- Run `nextm.exe` from any directory, desktop, or USB thumb drive.
- Create an empty **`nextm.ini`** file in the same folder to activate **Pure Portable Mode**: nextm will store all settings locally without writing to the Windows Registry.

---

## 🎮 Quick Controls & Shortcuts

| Action / Shortcut | Description |
|---|---|
| **Left Click** or **Double Click** (tray icon) | Toggles the **Unified Inspection Window** |
| **Mouse Hover** (tray icon) | Opens the compact floating telemetry summary card |
| **Right Click** (tray icon) | Opens the quick context menu (Inspect, Settings, Exit) |
| **F5** (in inspection window) | Manually forces an instant telemetry refresh |
| **Up / Down Arrows** | Navigates through list rows |
| **Left / Right Arrows** or **Space** | Expands or collapses process/socket hierarchy groups |
| **Del / Delete** | Terminates the selected process or closes active network connection |

---

## 🔬 Benchmarks & Real-World Footprint

Profiled using `cargo xtask bench` on Windows 11 with all telemetry modules active:

| Metric | Result | Description |
|---|---|---|
| **Binary Size** | ~1 MB | Includes all 5 views, custom raster fonts, icons & telemetry engine |
| **Private Commit (RAM)** | ~2.9 MB | Full telemetry, process trees & socket correlation running |
| **Private Working Set** | ~2.4 MB | Minimal physical RAM occupancy |
| **CPU Usage** | ~0.6% | Single logical core at idle base frequency |
| **Startup Time** | ~95 ms | Cold launch from double-click to ready tray |
| **Context Switches** | ~5.4 / s | Zero busy-wait loops, event-driven timers |
| **GDI Objects** | 0 persistent | Bitmaps and DC buffers strictly managed with RAII |
| **PE DLL Imports** | 15 native DLLs | Strictly native Windows/UCRT APIs; zero third-party DLLs |

---

## 🛠️ For Developers

### Prerequisites
- **Rust:** Recent stable toolchain (defined in `rust-toolchain.toml`).
- **Visual Studio Build Tools:** Windows SDK for the MSVC target (`x86_64-pc-windows-msvc` or `aarch64-pc-windows-msvc`).
- *(Optional)* **Inno Setup 6:** Required to compile the installer (`winget install JRSoftware.InnoSetup`).

### Common Commands
```powershell
# Build release binary
cargo build --release -p nextm

# Run complete workspace test suite (230+ tests)
cargo test --workspace

# Lint codebase (strict zero-warning policy)
cargo clippy --workspace --all-targets -- -D warnings

# Validate PE imports (ensures zero external DLL dependencies)
cargo xtask check-imports target/release/nextm.exe

# Validate binary size against established size budget
cargo xtask check-size target/release/nextm.exe --budget size-budget-x86_64-pc-windows-msvc.txt

# Build portable zip, installer exe, and generate SHA256SUMS.txt
cargo run -p xtask -- dist
```

---

## 👤 Author

**Lorenzo Pompili**  
- Email: [lorenzo.pompil@gmail.com](mailto:lorenzo.pompil@gmail.com)  
- Repository: [nextm on GitLab](https://gitlab.com/p3678/nextm)

---

## 📄 License

Distributed under the dual open-source licenses:
- [MIT License](LICENSE-MIT)
- [Apache License 2.0](LICENSE-APACHE)
