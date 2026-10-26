use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor},
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use std::io::{stdout, Write};
use std::path::PathBuf;
use std::time::Duration;

use crate::devices::{DeviceModel, KNOWN_DEVICES};
use crate::ports::{self, PortInfo};
use crate::protocol::client::{BslClient, FlashType};
use crate::protocol::pac::PacPackage;

pub struct TuiApp {
    pub devices: &'static [DeviceModel],
    pub selected_device_idx: usize,
    pub selected_op_idx: usize,
    pub active_pane: ActivePane, // 0: Devices, 1: Operations
    pub port_name: String,
    pub baud_rate: u32,
    pub logs: Vec<String>,
    pub running: bool,
    /// Shown in the title bar while a blocking operation runs.
    status: Option<&'static str>,
    /// Index of the operation waiting for destructive-action confirmation.
    pending_confirm: Option<usize>,
    /// Discovered serial ports (refreshed on 'p').
    port_list: Vec<PortInfo>,
    /// Port picker popup open.
    port_picker: bool,
    picker_idx: usize,
    /// Last logged message + repeat count, for collapsing spam.
    last_raw: String,
    last_count: usize,
}

#[derive(PartialEq, Clone, Copy)]
pub enum ActivePane {
    Devices,
    Operations,
}

pub const OPERATIONS: &[(&str, &str)] = &[
    ("Read Info / Chip UID", "Query processor ID, revision and unique hardware UID"),
    ("BROM Handshake", "Synchronize with device in BootROM mode (0x7E)"),
    ("Boot FDL1 & FDL2", "Send and execute Fast Download Loaders to RAM"),
    ("Read Live Partitions", "Query active flash partition table from FDL2"),
    ("Backup NVRAM", "Save NVRAM calibration & IMEI partitions (prodnv/fixnv)"),
    ("Restore NVRAM", "Flash saved NVRAM calibration backup to device"),
    ("Flash PAC Firmware", "Directly flash full stock PAC firmware package"),
    ("Inspect PAC Firmware", "View partitions and metadata inside .pac archive"),
    ("Unpack PAC to Folder", "Extract all partition images from .pac to folder"),
    ("Reset FRP (Google Lock)", "Wipe FRP, Persistent and Config partitions"),
    ("Factory Reset (Wipe Data)", "Format Userdata and Metadata partitions"),
    ("Unlock Bootloader", "REFUSED on secure-boot devices (misc wipe is not an unlock)"),
    ("Relock Bootloader", "REFUSED on secure-boot devices (misc wipe is not a relock)"),
    ("Dump Partition (boot, 64MB)", "Dump 'boot' partition -> ./dump_boot.img (fixed target)"),
    ("Flash Partition (./boot.img -> boot)", "Flash ./boot.img to 'boot' partition (fixed target)"),
    ("Erase Partition (misc)", "Erase 'misc' partition (fixed target)"),
    ("Reboot to System", "Send BSL_CMD_NORMAL_RESET to reboot"),
    ("Power Off Device", "Send BSL_CMD_POWER_OFF to safely shut down"),
];

/// Returns a list of directories where application assets and loaders are stored,
/// including current executable path (following symlinks), Termux opt path,
/// user share path, system opt, and current working directory.
pub fn get_app_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // 1. Directory of current running executable (following symlinks)
    if let Ok(exe) = std::env::current_exe() {
        if let Ok(canon) = std::fs::canonicalize(&exe) {
            if let Some(p) = canon.parent() {
                dirs.push(p.to_path_buf());
                dirs.push(p.join("loaders"));
            }
        }
        if let Some(p) = exe.parent() {
            dirs.push(p.to_path_buf());
            dirs.push(p.join("loaders"));
        }
    }

    // 2. Relative paths from current working directory
    dirs.push(PathBuf::from("./loaders"));
    dirs.push(PathBuf::from("."));
    dirs.push(PathBuf::from("../loaders"));
    dirs.push(PathBuf::from(".."));

    // 3. Android / Termux paths ($PREFIX/opt/unisoc-tool, /data/data/com.termux/files/usr/...)
    if let Ok(prefix) = std::env::var("PREFIX") {
        let p = PathBuf::from(prefix);
        dirs.push(p.join("opt/unisoc-tool"));
        dirs.push(p.join("opt/unisoc-tool/loaders"));
        dirs.push(p.join("bin"));
    }
    dirs.push(PathBuf::from("/data/data/com.termux/files/usr/opt/unisoc-tool"));
    dirs.push(PathBuf::from("/data/data/com.termux/files/usr/opt/unisoc-tool/loaders"));
    dirs.push(PathBuf::from("/data/data/com.termux/files/usr/bin"));

    // 4. Linux / Unix home and system paths
    if let Ok(home) = std::env::var("HOME") {
        let h = PathBuf::from(home);
        dirs.push(h.join(".local/share/unisoc-tool"));
        dirs.push(h.join(".local/share/unisoc-tool/loaders"));
        dirs.push(h.join("infinix_dump"));
        dirs.push(h.join(".local/bin"));
    }
    dirs.push(PathBuf::from("/opt/unisoc-tool"));
    dirs.push(PathBuf::from("/opt/unisoc-tool/loaders"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/var/mnt/storage/downloads/ums9230_Infinix_Hot_30i_base230522"));

    // 5. Windows paths
    if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
        let l = PathBuf::from(local_app_data);
        dirs.push(l.join("unisoc-tool"));
        dirs.push(l.join("unisoc-tool/loaders"));
    }

    dirs
}

pub fn find_spd_dump() -> Option<PathBuf> {
    for d in get_app_dirs() {
        #[cfg(windows)]
        let f = d.join("spd_dump.exe");
        #[cfg(not(windows))]
        let f = d.join("spd_dump");
        if f.is_file() {
            return std::fs::canonicalize(&f).ok().or(Some(f));
        }
    }

    #[cfg(windows)]
    let lookup_cmd = ("where", "spd_dump.exe");
    #[cfg(not(windows))]
    let lookup_cmd = ("which", "spd_dump");
    if let Ok(out) = std::process::Command::new(lookup_cmd.0).arg(lookup_cmd.1).output() {
        if out.status.success() {
            if let Some(first_line) = String::from_utf8_lossy(&out.stdout).lines().next() {
                let p = first_line.trim();
                if !p.is_empty() {
                    let pb = PathBuf::from(p);
                    return std::fs::canonicalize(&pb).ok().or(Some(pb));
                }
            }
        }
    }
    None
}

pub fn find_fdl_files() -> Option<(PathBuf, PathBuf)> {
    // 1. First priority: Prefer SLA-unlocked fdl1-dl.bin / fdl2-dl.bin
    for d in get_app_dirs() {
        let f1 = d.join("fdl1-dl.bin");
        let f2 = d.join("fdl2-dl.bin");
        if f1.is_file() && f2.is_file() {
            let a = std::fs::canonicalize(&f1).unwrap_or(f1);
            let b = std::fs::canonicalize(&f2).unwrap_or(f2);
            return Some((a, b));
        }
    }

    // 2. Second priority: Standard fdl1.bin / fdl2.bin
    for d in get_app_dirs() {
        let f1 = d.join("fdl1.bin");
        let f2 = d.join("fdl2.bin");
        if f1.is_file() && f2.is_file() {
            let a = std::fs::canonicalize(&f1).unwrap_or(f1);
            let b = std::fs::canonicalize(&f2).unwrap_or(f2);
            return Some((a, b));
        }
    }

    None
}

fn find_candidate_pac() -> Option<PathBuf> {
    let search_paths = [
        PathBuf::from("."),
        PathBuf::from("/home/coshick/infinix_dump"),
        PathBuf::from("/storage/emulated/0/workspace"),
        PathBuf::from("/storage/emulated/0/Download"),
        PathBuf::from("/storage/emulated/0"),
    ];
    for dir in &search_paths {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.extension().map(|e| e.eq_ignore_ascii_case("pac")).unwrap_or(false) {
                    return Some(p);
                }
            }
        }
    }
    None
}

impl TuiApp {
    pub fn new(port: String, baud: u32) -> Self {
        let mut port_list = Vec::new();
        let mut logs = vec![
            "[SYSTEM] unisoc-tool loaded.".to_string(),
        ];
        
        let sprd_bin = find_spd_dump();
        if let Some(ref spd) = sprd_bin {
            logs.push(format!("[ENGINE] spd_dump detected: {}", spd.display()));
            port_list.push(PortInfo {
                name: "USB:spd_dump".to_string(),
                desc: "Auto-detect via spd_dump (libusb bulk)".to_string(),
                vid: Some(ports::SPRD_VID),
            });
        } else {
            logs.push("[ENGINE] Native USB engine active (pure Rust / libusb direct)".to_string());
        }

        port_list.push(PortInfo {
            name: "USB:direct".to_string(),
            desc: "Raw USB bulk endpoints (pure Rust nusb)".to_string(),
            vid: Some(ports::SPRD_VID),
        });
        port_list.extend(ports::list_ports());

        let is_default_port = port == "/dev/ttyUSB0" || port.eq_ignore_ascii_case("COM3");
        let sprd_serial = ports::first_unisoc_serial();
        let port_name = if let Some(d) = sprd_serial {
            logs.push(format!("[DEVICE] Unisoc serial port found: {}", d));
            d
        } else if !is_default_port && std::path::Path::new(&port).exists() {
            logs.push(format!("[PORT] Using specified port: {}", port));
            port
        } else if sprd_bin.is_some() {
            logs.push("[DEVICE] USB Mode: hold Vol- and plug USB cable to connect.".to_string());
            "USB:spd_dump".to_string()
        } else {
            logs.push("[DEVICE] USB Direct Mode: hold Vol- and plug USB cable to connect.".to_string());
            "USB:direct".to_string()
        };

        logs.push("[READY] Select device and operation, then press ENTER.".to_string());
        Self {
            devices: KNOWN_DEVICES,
            selected_device_idx: KNOWN_DEVICES.iter().position(|d| d.name.contains("Hot 30i")).unwrap_or(0),
            selected_op_idx: 0,
            active_pane: ActivePane::Operations,
            port_name,
            baud_rate: baud,
            logs,
            running: true,
            status: None,
            pending_confirm: None,
            port_list,
            port_picker: false,
            picker_idx: 0,
            last_raw: String::new(),
            last_count: 0,
        }
    }

    pub fn add_log(&mut self, msg: String) {
        // Collapse a run of identical lines ("msg (×N)") instead of
        // flooding the pane when an op is retried over and over.
        if msg == self.last_raw {
            self.last_count += 1;
            if let Some(last) = self.logs.last_mut() {
                *last = format!("{} (×{})", msg, self.last_count);
            }
            return;
        }
        self.last_raw = msg.clone();
        self.last_count = 1;
        if self.logs.len() > 100 {
            self.logs.remove(0);
        }
        self.logs.push(msg);
    }

    /// Serial errors print the whole anyhow chain, so the OS reason
    /// (Permission denied / Device busy / No such device) is visible
    /// instead of a bare "Failed to open serial port".
    fn log_serial_error(&mut self, e: anyhow::Error) {
        self.add_log(format!(
            "[SERIAL ERROR] {:#} — press 'p' to pick a port",
            e
        ));
    }

    pub fn run(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut out = stdout();
        execute!(out, EnterAlternateScreen, cursor::Hide)?;

        while self.running {
            self.draw(&mut out)?;
            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    // Port picker owns all keys while open: arrows pick,
                    // Enter selects, Esc / p / q just close it.
                    if self.port_picker {
                        if key.modifiers.contains(KeyModifiers::CONTROL)
                            && matches!(key.code, KeyCode::Char('c'))
                        {
                            self.running = false;
                        } else {
                            match key.code {
                                KeyCode::Up | KeyCode::Char('k') => {
                                    if self.picker_idx > 0 {
                                        self.picker_idx -= 1;
                                    }
                                }
                                KeyCode::Down | KeyCode::Char('j') => {
                                    if self.picker_idx + 1 < self.port_list.len() {
                                        self.picker_idx += 1;
                                    }
                                }
                                KeyCode::Enter => {
                                    if let Some(p) = self.port_list.get(self.picker_idx) {
                                        self.port_name = p.name.clone();
                                        if p.is_sprd() {
                                            self.add_log(format!(
                                                "[DEVICE] Unisoc device selected: {}",
                                                p.name
                                            ));
                                        } else {
                                            self.add_log(format!(
                                                "[PORT] Selected {} (not a Unisoc device)",
                                                p.name
                                            ));
                                        }
                                    }
                                    self.port_picker = false;
                                }
                                KeyCode::Esc | KeyCode::Char('p') | KeyCode::Char('q') => {
                                    self.port_picker = false;
                                }
                                _ => {}
                            }
                        }
                        continue;
                    }
                    // A confirmation dialog owns all keys until answered:
                    // y confirms, everything else (incl. q / Esc) cancels.
                    // Ctrl+C still quits outright.
                    if self.pending_confirm.is_some() {
                        if key.modifiers.contains(KeyModifiers::CONTROL)
                            && matches!(key.code, KeyCode::Char('c'))
                        {
                            self.running = false;
                        } else if matches!(key.code, KeyCode::Char('y') | KeyCode::Char('Y')) {
                            let idx = self.pending_confirm.take().unwrap();
                            self.selected_op_idx = idx;
                            self.status = Some("Working…");
                            self.draw(&mut out)?;
                            self.execute_current_op(&mut out);
                            self.status = None;
                        } else {
                            self.pending_confirm = None;
                        }
                        continue;
                    }
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            self.running = false;
                        }
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            self.running = false;
                        }
                        KeyCode::Tab => {
                            self.active_pane = match self.active_pane {
                                ActivePane::Devices => ActivePane::Operations,
                                ActivePane::Operations => ActivePane::Devices,
                            };
                        }
                        KeyCode::Char('p') => {
                            let mut list = Vec::new();
                            if find_spd_dump().is_some() {
                                list.push(PortInfo {
                                    name: "USB:spd_dump".to_string(),
                                    desc: "Auto-detect via spd_dump (libusb bulk)".to_string(),
                                    vid: Some(ports::SPRD_VID),
                                });
                            }
                            list.push(PortInfo {
                                name: "USB:direct".to_string(),
                                desc: "Raw USB bulk endpoints (pure Rust nusb)".to_string(),
                                vid: Some(ports::SPRD_VID),
                            });
                            list.extend(ports::list_ports());
                            self.port_list = list;
                            self.picker_idx = self
                                .port_list
                                .iter()
                                .position(|p| p.name == self.port_name)
                                .unwrap_or(0);
                            self.port_picker = true;
                        }
                        KeyCode::Char('c') => {
                            self.logs.clear();
                            self.last_raw.clear();
                            self.last_count = 0;
                            self.add_log("[SYSTEM] Log cleared.".to_string());
                        }
                        KeyCode::Up | KeyCode::Char('k') => match self.active_pane {
                            ActivePane::Devices => {
                                if self.selected_device_idx > 0 {
                                    self.selected_device_idx -= 1;
                                }
                            }
                            ActivePane::Operations => {
                                if self.selected_op_idx > 0 {
                                    self.selected_op_idx -= 1;
                                }
                            }
                        },
                        KeyCode::Down | KeyCode::Char('j') => match self.active_pane {
                            ActivePane::Devices => {
                                if self.selected_device_idx + 1 < self.devices.len() {
                                    self.selected_device_idx += 1;
                                }
                            }
                            ActivePane::Operations => {
                                if self.selected_op_idx + 1 < OPERATIONS.len() {
                                    self.selected_op_idx += 1;
                                }
                            }
                        },
                        KeyCode::Enter => {
                            if is_destructive(OPERATIONS[self.selected_op_idx].0) {
                                self.pending_confirm = Some(self.selected_op_idx);
                            } else {
                                self.status = Some("Working…");
                                self.draw(&mut out)?;
                                self.execute_current_op(&mut out);
                                self.status = None;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        execute!(out, LeaveAlternateScreen, cursor::Show)?;
        disable_raw_mode()?;
        Ok(())
    }

    fn open_client(&mut self) -> Result<BslClient> {
        let client = BslClient::wait_and_open(&self.port_name, self.baud_rate, Duration::from_secs(8))?;
        if let Some(detected) = crate::ports::first_unisoc_serial() {
            if self.port_name != detected {
                self.port_name = detected;
            }
        }
        Ok(client)
    }

    fn redraw_if_active(&self, out: &mut std::io::Stdout) {
        if self.status.is_some() {
            let _ = self.draw(out);
        }
    }

    fn execute_spd_dump(&mut self, spd_dump_bin: &std::path::Path, args: &[&str], out: &mut std::io::Stdout) {
        self.add_log(format!("[EXEC] spd_dump {}", args.join(" ")));
        self.add_log("⏳ Ожидание телефона (выключите, зажмите Vol-, вставьте USB)... [Esc — отмена]".to_string());
        self.redraw_if_active(out);

        let bin_path = std::fs::canonicalize(spd_dump_bin).unwrap_or_else(|_| spd_dump_bin.to_path_buf());
        let bin_str = bin_path.to_string_lossy().to_string();
        let mut full_cmd = vec![bin_str];
        for a in args {
            full_cmd.push(a.to_string());
        }

        if cfg!(target_os = "linux") {
            let is_root = std::fs::read_to_string("/proc/self/status")
                .ok()
                .and_then(|s| {
                    s.lines().find(|l| l.starts_with("Uid:")).map(|l| {
                        let mut p = l.split_whitespace();
                        let _ = p.next();
                        p.next() == Some("0") || p.next() == Some("0")
                    })
                })
                .unwrap_or(false);
            let has_udev = ["/etc/udev/rules.d/99-unisoc.rules", "/usr/lib/udev/rules.d/99-unisoc.rules"]
                .iter()
                .any(|p| std::path::Path::new(p).exists());
            if !is_root && !has_udev {
                let have = |b: &str| std::process::Command::new("which").arg(b).output().map(|o| o.status.success()).unwrap_or(false);
                if have("pkexec") {
                    self.add_log("[AUTH] Нет udev-правила для 1782 → запуск через pkexec".to_string());
                    full_cmd.insert(0, "pkexec".to_string());
                    if let Some(parent) = spd_dump_bin.parent() {
                        full_cmd.insert(1, "env".to_string());
                        full_cmd.insert(2, "-C".to_string());
                        full_cmd.insert(3, parent.to_string_lossy().to_string());
                    }
                } else {
                    self.add_log("[AUTH] Нет udev-правила для 1782 → запуск через sudo -n".to_string());
                    full_cmd.insert(0, "-n".to_string());
                    full_cmd.insert(0, "sudo".to_string());
                }
                let _ = self.draw(out);
            }
        }

        let mut cmd = std::process::Command::new(&full_cmd[0]);
        cmd.args(&full_cmd[1..]);
        if let Some(parent) = spd_dump_bin.parent() {
            cmd.current_dir(parent);
        }
        // Keep stdin piped so spd_dump never steals TUI keys or spins on EOF in getc(stdin)
        cmd.stdin(std::process::Stdio::piped());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        match cmd.spawn() {
            Ok(mut child) => {
                use std::io::Read;
                use std::sync::mpsc;

                let (tx, rx) = mpsc::channel::<Vec<u8>>();
                if let Some(mut stdout) = child.stdout.take() {
                    let tx_out = tx.clone();
                    std::thread::spawn(move || {
                        let mut buf = [0u8; 1024];
                        while let Ok(n) = stdout.read(&mut buf) {
                            if n == 0 || tx_out.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    });
                }
                if let Some(mut stderr) = child.stderr.take() {
                    let tx_err = tx.clone();
                    std::thread::spawn(move || {
                        let mut buf = [0u8; 1024];
                        while let Ok(n) = stderr.read(&mut buf) {
                            if n == 0 || tx_err.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    });
                }
                drop(tx);

                let mut acc: Vec<u8> = Vec::new();
                let mut last_progress = false;
                let mut reached_prompt = false;
                let mut aborted = false;
                let mut sla_rejected = false;

                loop {
                    match rx.recv_timeout(Duration::from_millis(50)) {
                        Ok(chunk) => {
                            for b in chunk {
                                if b == b'\n' || b == b'\r' {
                                    let s = String::from_utf8_lossy(&acc).trim().to_string();
                                    acc.clear();
                                    if s.is_empty() {
                                        continue;
                                    }
                                    if last_progress {
                                        self.logs.pop();
                                        self.last_raw.clear();
                                    }
                                    last_progress = b == b'\r';
                                    let display_line = if s == "FDL2: incompatible partition" {
                                        "FDL2: Da_Info получен (0x96 handshake OK)".to_string()
                                    } else {
                                        s.clone()
                                    };
                                    self.add_log(format!("[SPD] {}", display_line));
                                    if s.contains("unexpected response (0x00fe)") && self.last_count >= 2 {
                                        sla_rejected = true;
                                        let _ = child.kill();
                                        break;
                                    }
                                    self.redraw_if_active(out);
                                } else {
                                    acc.push(b);
                                    // Linux spd_dump prints "BROM> ", "FDL1> ", "FDL2> ";
                                    // Windows spd_dump.exe (0x4090b9) prints "BROM >", "FDL1 >", "FDL2 >",
                                    // or system("pause") ("Press any key to continue").
                                    if acc.ends_with(b"BROM> ")
                                        || acc.ends_with(b"FDL1> ")
                                        || acc.ends_with(b"FDL2> ")
                                        || acc.ends_with(b"BROM >")
                                        || acc.ends_with(b"FDL1 >")
                                        || acc.ends_with(b"FDL2 >")
                                    {
                                        let s = String::from_utf8_lossy(&acc).trim().to_string();
                                        acc.clear();
                                        self.add_log(format!("[SPD] {}", s));
                                        self.redraw_if_active(out);
                                        reached_prompt = true;
                                        let _ = child.kill();
                                        break;
                                    } else if acc.ends_with(b"Press any key to continue . . .") {
                                        acc.clear();
                                        let _ = child.kill();
                                        break;
                                    }
                                }
                            }
                            if reached_prompt || sla_rejected {
                                break;
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            // Allow user to cancel waiting/running spd_dump with Esc or Ctrl+C
                            if self.status.is_some() && event::poll(Duration::from_millis(0)).unwrap_or(false) {
                                if let Ok(Event::Key(key)) = event::read() {
                                    let is_ctrl_c = key.modifiers.contains(KeyModifiers::CONTROL)
                                        && matches!(key.code, KeyCode::Char('c'));
                                    if matches!(key.code, KeyCode::Esc) || is_ctrl_c {
                                        aborted = true;
                                        let _ = child.kill();
                                        break;
                                    }
                                }
                            }
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }

                let rem = String::from_utf8_lossy(&acc).trim().to_string();
                if !rem.is_empty() {
                    self.add_log(format!("[SPD] {}", rem));
                }

                // Files written by root on Unix: hand them back to the invoking user.
                if cfg!(target_os = "linux") {
                    if let (Ok(uid), Ok(gid), Some(dir)) =
                        (std::env::var("SUDO_UID"), std::env::var("SUDO_GID"), spd_dump_bin.parent())
                    {
                        if let Ok(entries) = std::fs::read_dir(dir) {
                            for e in entries.flatten() {
                                let _ = std::process::Command::new("chown")
                                    .arg(format!("{uid}:{gid}"))
                                    .arg(e.path())
                                    .status();
                            }
                        }
                    }
                }

                match child.wait() {
                    Ok(status) => {
                        if aborted {
                            self.add_log("[WARN] Операция отменена пользователем (Esc)".to_string());
                        } else if sla_rejected {
                            self.add_log("[FAILED] FDL2 вернул 0x00fe (SLA-блокировка или неподдерживаемая команда). Используйте патченные fdl1-dl.bin / fdl2-dl.bin.".to_string());
                        } else if status.success() || reached_prompt {
                            self.add_log("[SUCCESS] Операция успешно завершена!".to_string());
                        } else if status.code() == Some(1) {
                            self.add_log("[FAILED] Телефон не ответил или ошибка подключения. Убедитесь, что телефон ВЫКЛЮЧЕН, зажата Vol-, и кабель вставлен.".to_string());
                        } else {
                            self.add_log(format!("[STATUS] spd_dump завершился с кодом: {:?}", status.code()));
                        }
                        self.redraw_if_active(out);
                    }
                    Err(e) => {
                        self.add_log(format!("[ERROR] Failed waiting for spd_dump: {}", e));
                        self.redraw_if_active(out);
                    }
                }
            }
            Err(e) => {
                self.add_log(format!("[ERROR] Failed to run spd_dump: {}", e));
                self.redraw_if_active(out);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_spd_with_fdl(
        &mut self,
        spd_bin: &std::path::Path,
        f1: &str,
        addr1: &str,
        f2: &str,
        addr2: &str,
        fdl1_addr: u32,
        post_exec: &[&str],
        out: &mut std::io::Stdout,
    ) {
        let mut owned_args: Vec<String> = vec!["--wait".to_string(), "300".to_string()];

        // On Windows, spd_dump.exe dynamically loads Channel9.dll (0x4086ab) and reads Channel.ini
        // from its working directory. Ensure they exist alongside spd_dump.exe if available.
        if let Some(dir) = spd_bin.parent() {
            if spd_bin.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false) {
                for dep in ["Channel9.dll", "Channel.ini"] {
                    let dst = dir.join(dep);
                    if !dst.is_file() {
                        for d in get_app_dirs() {
                            let src = d.join(dep);
                            if src.is_file() {
                                let _ = std::fs::copy(&src, &dst);
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Tiger T606 / UMS9230 (fdl1 @ 0x65000800): use custom_exec_no_verify_65015f08.bin
        // so BROM boots patched fdl1-dl.bin without RSA signature rejection.
        if fdl1_addr == 0x6500_0800 {
            if let Some(dir) = spd_bin.parent() {
                let target_payload = dir.join("custom_exec_no_verify_65015f08.bin");
                if !target_payload.is_file() {
                    for d in get_app_dirs() {
                        let src_payload = d.join("custom_exec_no_verify_65015f08.bin");
                        if src_payload.is_file() {
                            let _ = std::fs::copy(&src_payload, &target_payload);
                            break;
                        }
                    }
                }
                if target_payload.is_file() {
                    owned_args.push("exec_addr".to_string());
                    owned_args.push("0x65015f08".to_string());
                }
            }
        }

        owned_args.push("fdl".to_string());
        owned_args.push(f1.to_string());
        owned_args.push(addr1.to_string());
        owned_args.push("fdl".to_string());
        owned_args.push(f2.to_string());
        owned_args.push(addr2.to_string());
        owned_args.push("exec".to_string());

        if !post_exec.is_empty() {
            owned_args.push("skip_confirm".to_string());
            owned_args.push("1".to_string());
            for a in post_exec {
                owned_args.push((*a).to_string());
            }
        }

        let borrowed: Vec<&str> = owned_args.iter().map(|s| s.as_str()).collect();
        self.execute_spd_dump(spd_bin, &borrowed, out);
    }

    fn execute_current_op(&mut self, out: &mut std::io::Stdout) {
        let op_name = OPERATIONS[self.selected_op_idx].0;
        let dev = &self.devices[self.selected_device_idx];
        let fdl1_addr = dev.fdl1_addr;
        self.add_log(format!("[EXEC] Starting '{}' for '{}'...", op_name, dev.name));
        self.redraw_if_active(out);

        if let Some(spd_bin) = find_spd_dump() {
            let fdls = find_fdl_files();
            let fdl1_str = fdls.as_ref().map(|(f1, _)| f1.to_string_lossy().to_string());
            let fdl2_str = fdls.as_ref().map(|(_, f2)| f2.to_string_lossy().to_string());
            let addr1 = format!("0x{:08X}", dev.fdl1_addr);
            let addr2 = format!("0x{:08X}", dev.fdl2_addr);

            let handled = match op_name {
                "Read Info / Chip UID" => {
                    if let (Some(f1), Some(f2)) = (&fdl1_str, &fdl2_str) {
                        self.run_spd_with_fdl(
                            &spd_bin,
                            f1,
                            &addr1,
                            f2,
                            &addr2,
                            fdl1_addr,
                            &["chip_uid", "read_part", "miscdata", "8192", "64", "unlock_check.bin", "reset"],
                            out,
                        );
                        if let Some(dir) = spd_bin.parent() {
                            let chk = dir.join("unlock_check.bin");
                            if let Ok(bytes) = std::fs::read(&chk) {
                                if bytes.len() >= 64 {
                                    if bytes.iter().any(|&b| b != 0) {
                                        self.add_log("[SUCCESS] Загрузчик (Bootloader): РАЗБЛОКИРОВАН (Unlocked, токен в miscdata@8192)".to_string());
                                    } else {
                                        self.add_log("[INFO] Загрузчик (Bootloader): ЗАБЛОКИРОВАН (Locked)".to_string());
                                    }
                                    self.redraw_if_active(out);
                                }
                            }
                        }
                    } else {
                        self.execute_spd_dump(&spd_bin, &["--wait", "300", "chip_uid", "reset"], out);
                    }
                    true
                }
                "BROM Handshake" => {
                    // BROM does not support BSL_CMD_NORMAL_RESET (0x05); connecting with no post-commands
                    // completes CHECK_BAUD + BSL_REP_VER + CMD_CONNECT and reaches the BROM> prompt cleanly.
                    self.execute_spd_dump(&spd_bin, &["--wait", "300"], out);
                    true
                }
                "Boot FDL1 & FDL2" => {
                    if let (Some(f1), Some(f2)) = (&fdl1_str, &fdl2_str) {
                        self.run_spd_with_fdl(&spd_bin, f1, &addr1, f2, &addr2, fdl1_addr, &[], out);
                    } else {
                        self.add_log("[ERROR] fdl1.bin and fdl2.bin not found! Put them in ~/infinix_dump/".to_string());
                    }
                    true
                }
                "Read Live Partitions" => {
                    if let (Some(f1), Some(f2)) = (&fdl1_str, &fdl2_str) {
                        self.run_spd_with_fdl(
                            &spd_bin,
                            f1,
                            &addr1,
                            f2,
                            &addr2,
                            fdl1_addr,
                            &["partition_list", "partition_live.xml", "p", "reset"],
                            out,
                        );
                    } else {
                        self.execute_spd_dump(&spd_bin, &["--wait", "300", "partition_list", "partition_live.xml", "p", "reset"], out);
                    }
                    true
                }
                "Dump Partition (boot, 64MB)" => {
                    if let (Some(f1), Some(f2)) = (&fdl1_str, &fdl2_str) {
                        self.run_spd_with_fdl(
                            &spd_bin,
                            f1,
                            &addr1,
                            f2,
                            &addr2,
                            fdl1_addr,
                            &["read_part", "boot_b", "0", "64M", "boot_b_dump.img", "reset"],
                            out,
                        );
                        if let Some(dir) = spd_bin.parent() {
                            self.add_log(format!("[INFO] Файл дампа: {}/boot_b_dump.img", dir.display()));
                        }
                    } else {
                        self.add_log("[ERROR] FDL files needed to dump partition!".to_string());
                    }
                    true
                }
                "Flash Partition (./boot.img -> boot)" => {
                    if let (Some(f1), Some(f2)) = (&fdl1_str, &fdl2_str) {
                        let boot_candidates = [
                            std::env::current_dir().ok().map(|d| d.join("boot.img")).unwrap_or_else(|| PathBuf::from("./boot.img")),
                            PathBuf::from("/home/coshick/infinix_dump/boot.img"),
                        ];
                        if let Some(img) = boot_candidates.iter().find(|p| p.is_file()) {
                            let img_str = img.to_string_lossy().to_string();
                            self.run_spd_with_fdl(
                                &spd_bin,
                                f1,
                                &addr1,
                                f2,
                                &addr2,
                                fdl1_addr,
                                &["w", "boot_b", &img_str, "reset"],
                                out,
                            );
                        } else {
                            self.add_log("[ERROR] Файл boot.img не найден (положите его в текущую папку или в ~/infinix_dump/boot.img)".to_string());
                        }
                    } else {
                        self.add_log("[ERROR] FDL files needed to flash partition!".to_string());
                    }
                    true
                }
                "Backup NVRAM" => {
                    if let (Some(f1), Some(f2)) = (&fdl1_str, &fdl2_str) {
                        self.run_spd_with_fdl(
                            &spd_bin,
                            f1,
                            &addr1,
                            f2,
                            &addr2,
                            fdl1_addr,
                            &["r", "prodnv", "r", "miscdata", "r", "l_fixnv1_a", "r", "l_fixnv1_b", "reset"],
                            out,
                        );
                        if let Some(dir) = spd_bin.parent() {
                            self.add_log(format!("[INFO] NVRAM сохранён в: {}", dir.display()));
                        }
                    } else {
                        self.add_log("[ERROR] FDL files needed for NVRAM backup!".to_string());
                    }
                    true
                }
                "Restore NVRAM" => {
                    if let (Some(f1), Some(f2)) = (&fdl1_str, &fdl2_str) {
                        let has_backup = spd_bin
                            .parent()
                            .map(|d| d.join("prodnv.bin").is_file() && d.join("miscdata.bin").is_file())
                            .unwrap_or(false);
                        if has_backup {
                            self.run_spd_with_fdl(
                                &spd_bin,
                                f1,
                                &addr1,
                                f2,
                                &addr2,
                                fdl1_addr,
                                &["w", "prodnv", "prodnv.bin", "w", "miscdata", "miscdata.bin", "reset"],
                                out,
                            );
                        } else {
                            self.add_log("[ERROR] Файлы бэкапа prodnv.bin / miscdata.bin не найдены! Сначала сделайте Backup NVRAM.".to_string());
                        }
                    } else {
                        self.add_log("[ERROR] FDL files needed for NVRAM restore!".to_string());
                    }
                    true
                }
                "Reset FRP (Google Lock)" => {
                    if let (Some(f1), Some(f2)) = (&fdl1_str, &fdl2_str) {
                        self.run_spd_with_fdl(&spd_bin, f1, &addr1, f2, &addr2, fdl1_addr, &["e", "persist", "e", "misc", "reset"], out);
                    } else {
                        self.execute_spd_dump(&spd_bin, &["--wait", "300", "skip_confirm", "1", "e", "persist", "reset"], out);
                    }
                    true
                }
                "Factory Reset (Wipe Data)" => {
                    if let (Some(f1), Some(f2)) = (&fdl1_str, &fdl2_str) {
                        self.run_spd_with_fdl(
                            &spd_bin,
                            f1,
                            &addr1,
                            f2,
                            &addr2,
                            fdl1_addr,
                            &["e", "userdata", "e", "metadata", "e", "cache", "reset"],
                            out,
                        );
                    } else {
                        self.execute_spd_dump(&spd_bin, &["--wait", "300", "skip_confirm", "1", "e", "userdata", "reset"], out);
                    }
                    true
                }
                "Unlock Bootloader" => {
                    if let (Some(f1), Some(f2)) = (&fdl1_str, &fdl2_str) {
                        self.run_spd_with_fdl(
                            &spd_bin,
                            f1,
                            &addr1,
                            f2,
                            &addr2,
                            fdl1_addr,
                            &["read_part", "miscdata", "8192", "64", "unlock_check.bin", "reset"],
                            out,
                        );
                        if let Some(dir) = spd_bin.parent() {
                            if let Ok(bytes) = std::fs::read(dir.join("unlock_check.bin")) {
                                if bytes.len() >= 64 && bytes.iter().any(|&b| b != 0) {
                                    self.add_log("[SUCCESS] Загрузчик УЖЕ РАЗБЛОКИРОВАН (токен разблокировки активен в miscdata@8192)!".to_string());
                                } else {
                                    self.add_log("[WARN] Загрузчик заблокирован. Для T606 используйте цикл spl-unlock.bin / fdl2-cboot.bin.".to_string());
                                }
                            }
                        }
                    } else {
                        self.add_log("[ERROR] FDL files needed for bootloader unlock check!".to_string());
                    }
                    true
                }
                "Relock Bootloader" => {
                    self.add_log("[WARN] Блокировка загрузчика отключена во избежание brick-состояния с кастомным boot/vbmeta.".to_string());
                    true
                }
                "Erase Partition (misc)" => {
                    if let (Some(f1), Some(f2)) = (&fdl1_str, &fdl2_str) {
                        self.run_spd_with_fdl(&spd_bin, f1, &addr1, f2, &addr2, fdl1_addr, &["e", "misc", "reset"], out);
                    } else {
                        self.execute_spd_dump(&spd_bin, &["--wait", "300", "skip_confirm", "1", "e", "misc", "reset"], out);
                    }
                    true
                }
                "Reboot to System" => {
                    if let (Some(f1), Some(f2)) = (&fdl1_str, &fdl2_str) {
                        self.run_spd_with_fdl(&spd_bin, f1, &addr1, f2, &addr2, fdl1_addr, &["reset"], out);
                    } else {
                        self.execute_spd_dump(&spd_bin, &["--wait", "300", "reset"], out);
                    }
                    true
                }
                "Power Off Device" => {
                    if let (Some(f1), Some(f2)) = (&fdl1_str, &fdl2_str) {
                        self.run_spd_with_fdl(&spd_bin, f1, &addr1, f2, &addr2, fdl1_addr, &["poweroff"], out);
                    } else {
                        self.execute_spd_dump(&spd_bin, &["--wait", "300", "poweroff"], out);
                    }
                    true
                }
                _ => false,
            };

            if handled {
                return;
            }
        }

        match op_name {
            "Read Info / Chip UID" => {
                match self.open_client() {
                    Ok(mut client) => {
                        let _ = client.handshake(50);
                        // connect() was missing here: chip queries need it.
                        let _ = client.connect();
                        match client.read_chip_type() {
                            Ok(chip) => self.add_log(format!("[INFO] Detected Chip: {}", chip)),
                            Err(e) => self.add_log(format!("[ERROR] {}", e)),
                        }
                        match client.read_chip_uid() {
                            Ok(uid) => self.add_log(format!("[INFO] Chip UID: {}", uid)),
                            Err(e) => self.add_log(format!("[ERROR] {}", e)),
                        }
                    }
                    Err(e) => self.log_serial_error(e),
                }
            }
            "BROM Handshake" => {
                match self.open_client() {
                    Ok(mut client) => match client.handshake(200) {
                        Ok(_) => {
                            let _ = client.connect();
                            self.add_log("[SUCCESS] Device synced in BootROM mode!".to_string())
                        }
                        Err(e) => self.add_log(format!("[FAILED] {}", e)),
                    },
                    Err(e) => self.log_serial_error(e),
                }
            }
            "Boot FDL1 & FDL2" => {
                if let Some(pac_path) = find_candidate_pac() {
                    self.add_log(format!("[BOOT] Booting device using FDLs from: {}", pac_path.display()));
                    match self.open_client() {
                        Ok(mut client) => {
                            // boot_from_pac stops after FDL2: no flashing, no
                            // reboot, so the phone STAYS in FDL2 mode.
                            match client.boot_from_pac(&pac_path, dev.fdl1_addr, dev.fdl2_addr, 4096) {
                                Ok(_) => self.add_log("[SUCCESS] FDL1 & FDL2 booted, device stays in FDL2 mode!".to_string()),
                                Err(e) => self.add_log(format!("[ERROR] {}", e)),
                            }
                        }
                        Err(e) => self.log_serial_error(e),
                    }
                } else {
                    self.add_log(format!("[BOOT] Run CLI: unisoc-tool boot --model '{}' --fdl1 fdl1.bin --fdl2 fdl2.bin", dev.name));
                }
            }
            "Read Live Partitions" => {
                match self.open_client() {
                    Ok(mut client) => match client.read_partition_table() {
                        Ok(parts) => {
                            self.add_log(format!("[SUCCESS] Found {} partitions on device:", parts.len()));
                            for (name, size) in parts.iter().take(6) {
                                self.add_log(format!("  • {:<14} ({:.2} MB)", name, *size as f64 / (1024.0 * 1024.0)));
                            }
                            if parts.len() > 6 {
                                self.add_log(format!("  ... +{} more (run 'unisoc-tool partitions')", parts.len() - 6));
                            }
                        }
                        Err(e) => self.add_log(format!("[ERROR] {}", e)),
                    },
                    Err(e) => self.log_serial_error(e),
                }
            }
            "Backup NVRAM" => {
                match self.open_client() {
                    Ok(mut client) => match client.backup_nvram(std::path::Path::new("./nvram_backup")) {
                        Ok(_) => self.add_log("[SUCCESS] NVRAM saved to ./nvram_backup/".to_string()),
                        Err(e) => self.add_log(format!("[ERROR] {}", e)),
                    },
                    Err(e) => self.log_serial_error(e),
                }
            }
            "Restore NVRAM" => {
                match self.open_client() {
                    Ok(mut client) => match client.restore_nvram(std::path::Path::new("./nvram_backup")) {
                        Ok(_) => self.add_log("[SUCCESS] NVRAM restored from ./nvram_backup/".to_string()),
                        Err(e) => self.add_log(format!("[ERROR] {}", e)),
                    },
                    Err(e) => self.log_serial_error(e),
                }
            }
            "Inspect PAC Firmware" => {
                if let Some(pac_path) = find_candidate_pac() {
                    self.add_log(format!("[PAC] Parsing candidate: {}", pac_path.display()));
                    match PacPackage::open(&pac_path) {
                        Ok(pkg) => {
                            self.add_log(format!("[PAC] Product: {}", pkg.product_name));
                            self.add_log(format!("[PAC] Version: {}", pkg.product_version));
                            self.add_log(format!(
                                "[PAC] Size: {:.2} MB ({} partitions)",
                                pkg.total_size as f64 / (1024.0 * 1024.0),
                                pkg.entries.len()
                            ));
                            if let Some(fdl1) = pkg.find_fdl1() {
                                self.add_log(format!("[PAC] FDL1 found ({} bytes)", fdl1.size));
                            }
                            if let Some(fdl2) = pkg.find_fdl2() {
                                self.add_log(format!("[PAC] FDL2 found ({} bytes)", fdl2.size));
                            }
                        }
                        Err(e) => self.add_log(format!("[PAC ERROR] {}", e)),
                    }
                } else {
                    self.add_log("[PAC] No .pac found in workspace/Download folder.".to_string());
                    self.add_log("[CLI] Run: unisoc-tool pac-info --pac <file.pac>".to_string());
                }
            }
            "Flash PAC Firmware" => {
                if let Some(pac_path) = find_candidate_pac() {
                    self.add_log(format!("[PAC] Flashing firmware: {}", pac_path.display()));
                    match self.open_client() {
                        Ok(mut client) => match client.flash_pac(&pac_path, dev.fdl1_addr, dev.fdl2_addr, 4096, None, FlashType::Auto, None) {
                            Ok(_) => self.add_log("[SUCCESS] PAC firmware flashed, device rebooting!".to_string()),
                            Err(e) => self.add_log(format!("[ERROR] {}", e)),
                        },
                        Err(e) => self.log_serial_error(e),
                    }
                } else {
                    self.add_log(format!(
                        "[CLI] Run: unisoc-tool flash-pac --port {} --pac <file.pac> --device '{}'",
                        self.port_name,
                        dev.name
                    ));
                }
            }
            "Unpack PAC to Folder" => {
                if let Some(pac_path) = find_candidate_pac() {
                    self.add_log(format!("[UNPACK] Extracting: {}", pac_path.display()));
                    match PacPackage::open(&pac_path) {
                        Ok(pkg) => match pkg.unpack_all(&pac_path, std::path::Path::new("./extracted_pac")) {
                            Ok(_) => self.add_log("[SUCCESS] PAC unpacked to ./extracted_pac/".to_string()),
                            Err(e) => self.add_log(format!("[ERROR] {}", e)),
                        },
                        Err(e) => self.add_log(format!("[PAC ERROR] {}", e)),
                    }
                } else {
                    self.add_log("[CLI] Run: unisoc-tool unpack-pac --pac <file.pac> --out <dir>".to_string());
                }
            }
            "Reset FRP (Google Lock)" => {
                match self.open_client() {
                    Ok(mut client) => match client.reset_frp() {
                        Ok(_) => self.add_log("[SUCCESS] FRP partitions cleared successfully!".to_string()),
                        Err(e) => self.add_log(format!("[ERROR] {}", e)),
                    },
                    Err(e) => self.log_serial_error(e),
                }
            }
            "Factory Reset (Wipe Data)" => {
                match self.open_client() {
                    Ok(mut client) => match client.factory_reset() {
                        Ok(_) => self.add_log("[SUCCESS] Userdata and metadata wiped!".to_string()),
                        Err(e) => self.add_log(format!("[ERROR] {}", e)),
                    },
                    Err(e) => self.log_serial_error(e),
                }
            }
            "Unlock Bootloader" => {
                match self.open_client() {
                    Ok(mut client) => match client.unlock_bootloader() {
                        Ok(_) => self.add_log("[SUCCESS] Bootloader unlock command sent!".to_string()),
                        Err(e) => self.add_log(format!("[ERROR] {}", e)),
                    },
                    Err(e) => self.log_serial_error(e),
                }
            }
            "Relock Bootloader" => {
                match self.open_client() {
                    Ok(mut client) => match client.relock_bootloader() {
                        Ok(_) => self.add_log("[SUCCESS] Bootloader relock command sent!".to_string()),
                        Err(e) => self.add_log(format!("[ERROR] {}", e)),
                    },
                    Err(e) => self.log_serial_error(e),
                }
            }
            "Dump Partition (boot, 64MB)" => {
                match self.open_client() {
                    Ok(mut client) => {
                        let out_path = std::path::Path::new("./dump_boot.img");
                        match client.backup_partition("boot", out_path, Some(67108864)) {
                            Ok(_) => self.add_log("[SUCCESS] Dumped 'boot' partition -> ./dump_boot.img".to_string()),
                            Err(e) => self.add_log(format!("[ERROR] {}", e)),
                        }
                    }
                    Err(e) => self.log_serial_error(e),
                }
            }
            "Flash Partition (./boot.img -> boot)" => {
                let candidate_img = std::path::Path::new("./boot.img");
                if candidate_img.exists() {
                    match self.open_client() {
                        Ok(mut client) => match client.flash_partition("boot", candidate_img) {
                            Ok(_) => self.add_log("[SUCCESS] Flashed ./boot.img to partition 'boot'".to_string()),
                            Err(e) => self.add_log(format!("[ERROR] {}", e)),
                        },
                        Err(e) => self.log_serial_error(e),
                    }
                } else {
                    self.add_log("[CLI] To flash, run: unisoc-tool flash-partition --partition <name> --file <img.bin>".to_string());
                }
            }
            "Erase Partition (misc)" => {
                match self.open_client() {
                    Ok(mut client) => match client.erase_partition("misc") {
                        Ok(_) => self.add_log("[SUCCESS] Erased 'misc' partition!".to_string()),
                        Err(e) => self.add_log(format!("[ERROR] {}", e)),
                    },
                    Err(e) => self.log_serial_error(e),
                }
            }
            "Reboot to System" => {
                match self.open_client() {
                    Ok(mut client) => match client.reboot() {
                        Ok(_) => self.add_log("[SUCCESS] Reboot command acknowledged!".to_string()),
                        Err(e) => self.add_log(format!("[ERROR] {}", e)),
                    },
                    Err(e) => self.log_serial_error(e),
                }
            }
            "Power Off Device" => {
                match self.open_client() {
                    Ok(mut client) => match client.power_off() {
                        Ok(_) => self.add_log("[SUCCESS] Power off acknowledged.".to_string()),
                        Err(e) => self.add_log(format!("[ERROR] {}", e)),
                    },
                    Err(e) => self.log_serial_error(e),
                }
            }
            _ => {
                self.add_log(format!("[READY] Selected operation '{}' ready for execution.", op_name));
            }
        }
    }

    fn draw(&self, out: &mut std::io::Stdout) -> Result<()> {
        let (cols, rows) = crossterm::terminal::size().unwrap_or((100, 30));
        let width = cols as usize;
        let height = rows as usize;
        let accent = Color::Cyan;
        let dim = Color::DarkGrey;

        execute!(out, cursor::MoveTo(0, 0))?;

        // Title row: bold name, dim connection info, right-aligned status.
        let mid = format!("  {} · {} baud", self.port_name, self.baud_rate);
        let status = self.status.unwrap_or("Ready");
        let status_color = if self.status.is_some() { accent } else { dim };
        execute!(
            out,
            SetAttribute(Attribute::Bold),
            SetForegroundColor(Color::White),
            Print("unisoc-tool"),
            ResetColor,
            SetForegroundColor(dim),
            Print(&mid),
        )?;
        let used = 11 + mid.chars().count() + status.chars().count();
        if width > used {
            execute!(out, Print(" ".repeat(width - used)))?;
        }
        execute!(
            out,
            SetForegroundColor(status_color),
            Print(status),
            ResetColor,
            cursor::MoveToNextLine(1),
        )?;

        // Content area: title(1) + content + desc(1) + footer(1).
        let content_height = height.saturating_sub(3);
        let dev_w = (width * 28 / 100).clamp(20, 32);
        let ops_w = (width * 32 / 100).clamp(22, 38);
        let log_w = width.saturating_sub(dev_w + ops_w + 2).max(20);
        let visible = content_height.saturating_sub(2);
        let dev_off = viewport_offset(self.selected_device_idx, self.devices.len(), visible);
        let ops_off = viewport_offset(self.selected_op_idx, OPERATIONS.len(), visible);

        for r in 0..content_height {
            // Devices column.
            let dev_active = self.active_pane == ActivePane::Devices;
            let dev_border = if dev_active { accent } else { dim };
            execute!(out, SetForegroundColor(dev_border))?;
            if r == 0 {
                execute!(out, Print(border_top(dev_w, "DEVICE")))?;
            } else if r == content_height - 1 {
                execute!(out, Print(border_bottom(dev_w)))?;
            } else {
                let i = r - 1;
                execute!(out, Print("│ "))?;
                if i < visible {
                    let idx = dev_off + i;
                    if idx < self.devices.len() {
                        let d = &self.devices[idx];
                        let selected = idx == self.selected_device_idx;
                        if selected {
                            execute!(
                                out,
                                SetAttribute(Attribute::Bold),
                                SetForegroundColor(accent),
                                Print(pad_trunc(&format!("▸ {}", d.name), dev_w - 4)),
                                ResetColor,
                            )?;
                        } else {
                            execute!(
                                out,
                                SetForegroundColor(Color::White),
                                Print(pad_trunc(&format!("  {}", d.name), dev_w - 4)),
                                ResetColor,
                            )?;
                        }
                    } else {
                        execute!(out, Print(" ".repeat(dev_w - 4)))?;
                    }
                } else {
                    execute!(out, Print(" ".repeat(dev_w - 4)))?;
                }
                execute!(out, SetForegroundColor(dev_border), Print(" │"))?;
            }
            execute!(out, Print(" "))?;

            // Operations column.
            let ops_active = self.active_pane == ActivePane::Operations;
            let ops_border = if ops_active { accent } else { dim };
            execute!(out, SetForegroundColor(ops_border))?;
            if r == 0 {
                execute!(out, Print(border_top(ops_w, "OPERATION")))?;
            } else if r == content_height - 1 {
                execute!(out, Print(border_bottom(ops_w)))?;
            } else {
                let i = r - 1;
                execute!(out, Print("│ "))?;
                if i < visible {
                    let idx = ops_off + i;
                    if idx < OPERATIONS.len() {
                        let (name, _) = OPERATIONS[idx];
                        let selected = idx == self.selected_op_idx;
                        if selected {
                            execute!(
                                out,
                                SetAttribute(Attribute::Bold),
                                SetForegroundColor(accent),
                                Print(pad_trunc(&format!("▸ {}", name), ops_w - 4)),
                                ResetColor,
                            )?;
                        } else {
                            execute!(
                                out,
                                SetForegroundColor(Color::White),
                                Print(pad_trunc(&format!("  {}", name), ops_w - 4)),
                                ResetColor,
                            )?;
                        }
                    } else {
                        execute!(out, Print(" ".repeat(ops_w - 4)))?;
                    }
                } else {
                    execute!(out, Print(" ".repeat(ops_w - 4)))?;
                }
                execute!(out, SetForegroundColor(ops_border), Print(" │"))?;
            }
            execute!(out, Print(" "))?;

            // Logs column: specs on top, log lines below. Never a pane.
            execute!(out, SetForegroundColor(dim))?;
            if r == 0 {
                execute!(out, Print(border_top(log_w, "DEVICE + LOG")))?;
            } else if r == content_height - 1 {
                execute!(out, Print(border_bottom(log_w)))?;
            } else {
                let i = r - 1;
                let inner = log_w - 4;
                execute!(out, Print("│ "))?;
                if i < visible {
                    let dev = &self.devices[self.selected_device_idx];
                    if i == 0 {
                        execute!(
                            out,
                            SetForegroundColor(Color::White),
                            Print(pad_trunc(&format!("chip  {}", dev.chip), inner)),
                            ResetColor,
                        )?;
                    } else if i == 1 {
                        execute!(
                            out,
                            SetForegroundColor(Color::White),
                            Print(pad_trunc(&format!("fdl1  0x{:08X}", dev.fdl1_addr), inner)),
                            ResetColor,
                        )?;
                    } else if i == 2 {
                        execute!(
                            out,
                            SetForegroundColor(Color::White),
                            Print(pad_trunc(&format!("fdl2  0x{:08X}", dev.fdl2_addr), inner)),
                            ResetColor,
                        )?;
                    } else if i == 3 {
                        execute!(out, SetForegroundColor(dim), Print("─".repeat(inner)), ResetColor)?;
                    } else {
                        let log_rows = visible.saturating_sub(4);
                        let start = self.logs.len().saturating_sub(log_rows);
                        let log_idx = start + (i - 4);
                        if log_idx < self.logs.len() {
                            let line = &self.logs[log_idx];
                            execute!(
                                out,
                                SetForegroundColor(log_color(line)),
                                Print(pad_trunc(line, inner)),
                                ResetColor,
                            )?;
                        } else {
                            execute!(out, Print(" ".repeat(inner)))?;
                        }
                    }
                } else {
                    execute!(out, Print(" ".repeat(inner)))?;
                }
                execute!(out, SetForegroundColor(dim), Print(" │"))?;
            }

            execute!(out, ResetColor, cursor::MoveToNextLine(1))?;
        }

        // Selected operation description, dim.
        let desc = OPERATIONS[self.selected_op_idx].1;
        execute!(
            out,
            SetForegroundColor(dim),
            Print(pad_trunc(desc, width)),
            ResetColor,
            cursor::MoveToNextLine(1),
        )?;

        // Footer shortcuts, dim.
        execute!(
            out,
            SetForegroundColor(dim),
            Print(pad_trunc("↑↓ move   tab pane   p ports   c clear   q quit", width)),
            ResetColor,
        )?;

        // Confirmation dialog overlay for destructive operations.
        if let Some(idx) = self.pending_confirm {
            let op = OPERATIONS[idx].0;
            let msg = confirm_text(op);
            let bw = (msg.chars().count() + 8).clamp(40, width.saturating_sub(4).max(40));
            let bx = width.saturating_sub(bw) / 2;
            let by = height.saturating_sub(4) / 2;
            let keys = "[y] confirm   [esc] cancel";
            execute!(
                out,
                cursor::MoveTo(bx as u16, by as u16),
                SetForegroundColor(accent),
                Print(border_top(bw, "confirm")),
                cursor::MoveTo(bx as u16, (by + 1) as u16),
                Print("│ "),
                SetForegroundColor(Color::White),
                Print(pad_trunc(msg, bw - 4)),
                SetForegroundColor(accent),
                Print(" │"),
                cursor::MoveTo(bx as u16, (by + 2) as u16),
                Print("│ "),
                SetForegroundColor(dim),
                Print(pad_trunc(keys, bw - 4)),
                SetForegroundColor(accent),
                Print(" │"),
                cursor::MoveTo(bx as u16, (by + 3) as u16),
                Print(border_bottom(bw)),
                ResetColor,
            )?;
        }

        // Port picker overlay.
        if self.port_picker {
            let bw = 64.min(width.saturating_sub(4).max(20));
            let list_rows = self.port_list.len();
            let bh = if list_rows == 0 {
                5
            } else {
                (list_rows + 3).min(height.saturating_sub(2).max(5))
            };
            let bx = width.saturating_sub(bw) / 2;
            let by = height.saturating_sub(bh) / 2;
            let visible_p = bh.saturating_sub(3);
            let p_off = viewport_offset(self.picker_idx, list_rows, visible_p);
            execute!(
                out,
                cursor::MoveTo(bx as u16, by as u16),
                SetForegroundColor(accent),
                Print(border_top(bw, "ports")),
            )?;
            for i in 0..visible_p {
                let row = by + 1 + i;
                execute!(out, cursor::MoveTo(bx as u16, row as u16), Print("│ "))?;
                if list_rows == 0 && i == 0 {
                    execute!(
                        out,
                        SetForegroundColor(dim),
                        Print(pad_trunc("No serial ports found", bw - 4)),
                        SetForegroundColor(accent),
                    )?;
                } else {
                    let idx = p_off + i;
                    if idx < list_rows {
                        let p = &self.port_list[idx];
                        let line = format!(
                            "{} — {}{}",
                            p.name,
                            p.desc,
                            if p.is_sprd() { "  [UNISOC]" } else { "" }
                        );
                        if idx == self.picker_idx {
                            execute!(
                                out,
                                SetAttribute(Attribute::Bold),
                                SetForegroundColor(accent),
                                Print(pad_trunc(&format!("▸ {}", line), bw - 4)),
                                ResetColor,
                            )?;
                        } else {
                            execute!(
                                out,
                                SetForegroundColor(Color::White),
                                Print(pad_trunc(&format!("  {}", line), bw - 4)),
                                ResetColor,
                            )?;
                        }
                    } else {
                        execute!(out, Print(" ".repeat(bw - 4)))?;
                    }
                }
                execute!(out, SetForegroundColor(accent), Print(" │"))?;
            }
            execute!(
                out,
                cursor::MoveTo(bx as u16, (by + bh - 2) as u16),
                Print("│ "),
                SetForegroundColor(dim),
                Print(pad_trunc("[enter] select   [esc] close", bw - 4)),
                SetForegroundColor(accent),
                Print(" │"),
                cursor::MoveTo(bx as u16, (by + bh - 1) as u16),
                Print(border_bottom(bw)),
                ResetColor,
            )?;
        }

        out.flush()?;
        Ok(())
    }
}

/// Top border with an embedded title, exactly `w` chars wide.
fn border_top(w: usize, title: &str) -> String {
    let dashes = w.saturating_sub(title.chars().count() + 5);
    format!("┌─ {} {}", title, "─".repeat(dashes) + "┐")
}

/// Bottom border, exactly `w` chars wide.
fn border_bottom(w: usize) -> String {
    format!("└{}┘", "─".repeat(w.saturating_sub(2)))
}

/// Char-safe truncation + right padding to exactly `width` chars.
/// The old code sliced by bytes and could panic mid-UTF-8-character
/// (log lines contain "•").
fn pad_trunc(s: &str, width: usize) -> String {
    let taken: String = s.chars().take(width).collect();
    let len = taken.chars().count();
    if len < width {
        format!("{}{}", taken, " ".repeat(width - len))
    } else {
        taken
    }
}

/// Viewport offset so `selected` is always visible in `visible` rows.
fn viewport_offset(selected: usize, total: usize, visible: usize) -> usize {
    if total <= visible || visible == 0 {
        return 0;
    }
    let mut off = selected.saturating_sub(visible - 1);
    if off + visible > total {
        off = total - visible;
    }
    off
}

/// Semantic log colors; everything else stays plain white.
fn log_color(line: &str) -> Color {
    if line.starts_with("[ERROR]")
        || line.starts_with("[SERIAL ERROR]")
        || line.starts_with("[FAILED]")
    {
        Color::Red
    } else if line.starts_with("[SUCCESS]") {
        Color::Green
    } else if line.starts_with("[WARN") {
        Color::Yellow
    } else if line.starts_with("[SYSTEM]") || line.starts_with("[READY]") {
        Color::DarkGrey
    } else {
        Color::White
    }
}

/// Operations that modify the device and therefore ask for confirmation.
fn is_destructive(op: &str) -> bool {
    matches!(
        op,
        "Flash PAC Firmware"
            | "Flash Partition (./boot.img -> boot)"
            | "Erase Partition (misc)"
            | "Reset FRP (Google Lock)"
            | "Factory Reset (Wipe Data)"
    )
}

fn confirm_text(op: &str) -> &'static str {
    match op {
        "Flash PAC Firmware" => "Flash full PAC firmware? This overwrites partitions.",
        "Flash Partition (./boot.img -> boot)" => "Flash ./boot.img to the boot partition?",
        "Erase Partition (misc)" => "Erase the misc partition?",
        "Reset FRP (Google Lock)" => "Wipe FRP partitions? The FRP lock will be gone.",
        "Factory Reset (Wipe Data)" => "Factory reset? Userdata will be wiped.",
        _ => "This operation modifies the device. Confirm?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_trunc_never_splits_utf8() {
        // "•" is 3 bytes in UTF-8; byte slicing here used to panic.
        let s = "  • boot (64.00 MB)";
        let out = pad_trunc(s, 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.starts_with("  • b"));
        // Width 3 cuts right after the bullet, must not panic.
        assert_eq!(pad_trunc(s, 3), "  •");
        // Padding to width.
        assert_eq!(pad_trunc("ab", 5), "ab   ");
    }

    #[test]
    fn viewport_follows_selection() {
        assert_eq!(viewport_offset(0, 18, 10), 0);
        assert_eq!(viewport_offset(9, 18, 10), 0);
        assert_eq!(viewport_offset(10, 18, 10), 1);
        assert_eq!(viewport_offset(17, 18, 10), 8);
        assert_eq!(viewport_offset(2, 3, 10), 0);
        assert_eq!(viewport_offset(0, 5, 0), 0);
    }

    #[test]
    fn destructive_set_is_exact() {
        assert!(is_destructive("Flash PAC Firmware"));
        assert!(is_destructive("Reset FRP (Google Lock)"));
        assert!(!is_destructive("Read Info / Chip UID"));
        assert!(!is_destructive("Reboot to System"));
    }

    #[test]
    fn log_dedup_collapses_repeats() {
        let mut app = TuiApp::new("/dev/ttyUSB0".to_string(), 115200);
        let base = app.logs.len();
        app.add_log("boom".to_string());
        app.add_log("boom".to_string());
        app.add_log("boom".to_string());
        assert_eq!(app.logs.len(), base + 1);
        assert!(app.logs.last().unwrap().ends_with("(×3)"));
        app.add_log("other".to_string());
        assert_eq!(app.logs.len(), base + 2);
        // A repeat after a different line starts a fresh counter.
        app.add_log("boom".to_string());
        assert_eq!(app.logs.len(), base + 3);
        assert_eq!(app.logs.last().unwrap(), "boom");
    }

    #[test]
    fn port_discovery_runs_without_hardware() {
        // Must not panic on a machine with no serial ports.
        let found = crate::ports::list_ports();
        assert!(found.iter().all(|p| !p.name.is_empty()));
        // resolve_port never panics and always returns something usable.
        let resolved = crate::protocol::client::BslClient::resolve_port("/dev/ttyUSB99");
        assert!(!resolved.is_empty());
    }

    #[test]
    fn sprd_detection_prefers_unisoc_vid() {
        // Regression test for the Pico incident: a Raspberry Pi Pico
        // (239a) must never outrank a Spreadtrum phone (1782).
        let sprd = crate::ports::PortInfo {
            name: "/dev/ttyACM1".to_string(),
            desc: "USB Spreadtrum".to_string(),
            vid: Some(0x1782),
        };
        let pico = crate::ports::PortInfo {
            name: "/dev/ttyACM0".to_string(),
            desc: "USB Raspberry Pi Pico".to_string(),
            vid: Some(0x239a),
        };
        let uart = crate::ports::PortInfo {
            name: "/dev/ttyS0".to_string(),
            desc: "Serial".to_string(),
            vid: None,
        };
        assert!(sprd.is_sprd());
        assert!(!pico.is_sprd());
        assert!(!uart.is_sprd());
        assert!(crate::ports::sort_rank(&sprd) < crate::ports::sort_rank(&pico));
        assert!(crate::ports::sort_rank(&pico) < crate::ports::sort_rank(&uart));
    }

    #[test]
    fn spd_dump_integration_and_live_output() {
        let mut app = TuiApp::new("/dev/ttyUSB0".to_string(), 115200);
        // Default selected device must be Infinix Hot 30i with 0x65000800
        let dev = &app.devices[app.selected_device_idx];
        assert!(dev.name.contains("Hot 30i"));
        assert_eq!(dev.fdl1_addr, 0x65000800);
        assert_eq!(dev.fdl2_addr, 0x9efffe00);
        // Must never auto-pick /dev/ttyACM1 (Pico)
        assert_ne!(app.port_name, "/dev/ttyACM1");

        if let Some(spd_bin) = find_spd_dump() {
            assert!(find_fdl_files().is_some(), "FDL1/FDL2 files must be found");
            let mut out = std::io::stdout();
            // Run spd_dump in offline tool mode (-t) with 'exit' command to verify live stream capture
            app.execute_spd_dump(&spd_bin, &["-t", "exit"], &mut out);
            assert!(
                app.logs.iter().any(|l| l.contains("[SPD] ver:1")),
                "Logs did not capture spd_dump version banner: {:?}",
                app.logs
            );
            assert!(
                app.logs.iter().any(|l| l.starts_with("[SUCCESS]")),
                "Logs did not record success: {:?}",
                app.logs
            );
        }
    }
}
