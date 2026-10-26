//! Serial port discovery.
//!
//! Lists real serial ports the OS knows about (udev/sysfs on Linux)
//! instead of existence-checking a hardcoded list of paths. USB VID/PID
//! and product strings come from sysfs directly, so this works even
//! though the `serialport` dependency is built without its
//! `usb-port-info` feature.
//!
//! The whole point of the VID is telling a Unisoc phone apart from
//! other USB-serial gadgets: a Pico, an Arduino or a debug probe must
//! never win auto-detection over a Spreadtrum/Unisoc device.

use std::path::Path;
use std::time::Duration;

/// Spreadtrum/Unisoc USB vendor ID. A phone in BootROM/download mode
/// enumerates with this VID.
pub const SPRD_VID: u16 = 0x1782;

#[derive(Debug, Clone)]
pub struct PortInfo {
    pub name: String,
    pub desc: String,
    pub vid: Option<u16>,
}

impl PortInfo {
    pub fn is_sprd(&self) -> bool {
        self.vid == Some(SPRD_VID) || is_sprd_description(&self.desc)
    }
}

/// Checks if a port/USB description string matches known Windows/Linux Spreadtrum/Unisoc
/// driver names (e.g. "SPRD U2S Diag", "Spreadtrum", "SCI USB2Serial", "UNISOC").
pub(crate) fn is_sprd_description(desc: &str) -> bool {
    let up = desc.to_ascii_uppercase();
    up.contains("SPRD")
        || up.contains("SPREADTRUM")
        || up.contains("UNISOC")
        || up.contains("U2S DIAG")
        || up.contains("SCI USB2SERIAL")
        || up.contains("VID_1782")
        || up.contains("1782:")
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct RawUsbDevice {
    pub sysfs_name: String,
    pub vid: u16,
    pub pid: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub has_tty: bool,
    pub tty_name: Option<String>,
}

/// Sort rank: SPRD devices first, then USB/ACM/COM, then the rest.
pub(crate) fn sort_rank(p: &PortInfo) -> u8 {
    if p.is_sprd() {
        0
    } else if p.name.contains("USB") || p.name.contains("ACM") || p.name.starts_with("COM") {
        1
    } else {
        2
    }
}

/// Scans all USB devices directly via `/sys/bus/usb/devices/` on Linux,
/// or via `nusb::list_devices()` on Windows/macOS.
/// Finds devices regardless of whether a serial/tty driver has attached to them.
pub fn scan_raw_usb_devices() -> Vec<RawUsbDevice> {
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/sys/bus/usb/devices") {
        for entry in entries.flatten() {
            let path = entry.path();
            let vid_path = path.join("idVendor");
            let pid_path = path.join("idProduct");
            if !vid_path.exists() || !pid_path.exists() {
                continue;
            }
            let vid_str = std::fs::read_to_string(&vid_path).unwrap_or_default();
            let pid_str = std::fs::read_to_string(&pid_path).unwrap_or_default();
            let Ok(vid) = u16::from_str_radix(vid_str.trim(), 16) else {
                continue;
            };
            let Ok(pid) = u16::from_str_radix(pid_str.trim(), 16) else {
                continue;
            };

            let manufacturer = std::fs::read_to_string(path.join("manufacturer"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let product = std::fs::read_to_string(path.join("product"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());

            let mut has_tty = false;
            let mut tty_name = None;

            if let Ok(children) = std::fs::read_dir(&path) {
                for child in entries_flatten(children) {
                    let tty_dir = child.path().join("tty");
                    if tty_dir.is_dir() {
                        if let Ok(tty_entries) = std::fs::read_dir(&tty_dir) {
                            for tty_ent in tty_entries.flatten() {
                                has_tty = true;
                                tty_name = Some(format!("/dev/{}", tty_ent.file_name().to_string_lossy()));
                                break;
                            }
                        }
                    }
                }
            }

            results.push(RawUsbDevice {
                sysfs_name: entry.file_name().to_string_lossy().to_string(),
                vid,
                pid,
                manufacturer,
                product,
                has_tty,
                tty_name,
            });
        }
        return results;
    }

    // Cross-platform fallback (Windows / macOS) via nusb
    use nusb::MaybeFuture;
    if let Ok(list) = nusb::list_devices().wait() {
        let sprd_com = first_unisoc_serial();
        for (idx, dev) in list.enumerate() {
            let vid = dev.vendor_id();
            let pid = dev.product_id();
            let is_unisoc = vid == SPRD_VID;
            results.push(RawUsbDevice {
                sysfs_name: format!("usb#{idx}"),
                vid,
                pid,
                manufacturer: dev.manufacturer_string().map(|s| s.to_string()),
                product: dev.product_string().map(|s| s.to_string()),
                has_tty: is_unisoc && sprd_com.is_some(),
                tty_name: if is_unisoc { sprd_com.clone() } else { None },
            });
        }
    }
    results
}

fn entries_flatten(rd: std::fs::ReadDir) -> impl Iterator<Item = std::fs::DirEntry> {
    rd.flatten()
}

/// Filter raw USB devices down to Spreadtrum / Unisoc (VID 0x1782).
pub fn scan_raw_unisoc_devices() -> Vec<RawUsbDevice> {
    scan_raw_usb_devices()
        .into_iter()
        .filter(|d| d.vid == SPRD_VID)
        .collect()
}

/// Attempts to automatically bind the Linux `option` USB-serial driver to a Unisoc device.
/// Returns true if successful.
#[allow(dead_code)]
pub fn try_auto_bind_unisoc(pid: u16) -> bool {
    let new_id_path = Path::new("/sys/bus/usb-serial/drivers/option1/new_id");
    if !new_id_path.exists() {
        let _ = std::process::Command::new("modprobe").arg("option").output();
    }
    if new_id_path.exists() {
        let entry = format!("1782 {:04x}\n", pid);
        if std::fs::write(new_id_path, entry.as_bytes()).is_ok() {
            std::thread::sleep(Duration::from_millis(250));
            return true;
        }
    }
    false
}

/// All serial ports the OS reports, with a human-readable description.
/// Sorted so a Unisoc device comes first, then other USB-serial ports,
/// then the rest (built-in UARTs and such).
pub fn list_ports() -> Vec<PortInfo> {
    let Ok(ports) = serialport::available_ports() else {
        return Vec::new();
    };
    let mut out: Vec<PortInfo> = ports
        .into_iter()
        .map(|p| {
            let short = p.port_name.rsplit('/').next().unwrap_or(&p.port_name);
            let (desc, vid) = match &p.port_type {
                serialport::SerialPortType::UsbPort(info) => {
                    let mut d = String::from("USB");
                    if let Some(m) = &info.manufacturer {
                        d.push_str(&format!(" {m}"));
                    }
                    if let Some(pr) = &info.product {
                        d.push_str(&format!(" {pr}"));
                    }
                    d.push_str(&format!(" ({:04x}:{:04x})", info.vid, info.pid));
                    (d, Some(info.vid))
                }
                serialport::SerialPortType::BluetoothPort => ("Bluetooth".to_string(), None),
                serialport::SerialPortType::PciPort => ("PCI".to_string(), None),
                _ => match sysfs_usb(short) {
                    Some((v, pid, manuf, prod)) => {
                        let mut d = String::from("USB");
                        if let Some(m) = manuf {
                            d.push_str(&format!(" {m}"));
                        }
                        if let Some(pr) = prod {
                            d.push_str(&format!(" {pr}"));
                        }
                        d.push_str(&format!(" ({v:04x}:{pid:04x})"));
                        (d, Some(v))
                    }
                    None => ("Serial".to_string(), None),
                },
            };
            PortInfo {
                name: p.port_name,
                desc,
                vid,
            }
        })
        .collect();
    // Stable sort: SPRD devices first, then USB/ACM, then the rest.
    out.sort_by_key(sort_rank);
    out
}

/// First discovered Unisoc/Spreadtrum device, if one is connected.
pub fn first_unisoc_serial() -> Option<String> {
    list_ports()
        .into_iter()
        .find(|p| p.is_sprd())
        .map(|p| p.name)
}

/// First discovered port whose name looks like a USB-serial adapter —
/// the kind a phone in BSL/download mode enumerates as (ttyUSB*, ttyACM*).
/// Only a fallback: prefer [`first_unisoc_serial`].
#[allow(dead_code)]
pub fn first_usb_serial() -> Option<String> {
    list_ports()
        .into_iter()
        .find(|p| p.name.contains("USB") || p.name.contains("ACM"))
        .map(|p| p.name)
}

/// USB identity of a tty, read from sysfs by walking up from
/// /sys/class/tty/<name> to the owning USB device.
/// Returns (vid, pid, manufacturer, product); None when the tty is not
/// on a USB device (or sysfs is absent).
fn sysfs_usb(tty_name: &str) -> Option<(u16, u16, Option<String>, Option<String>)> {
    let mut dir = std::fs::canonicalize(format!("/sys/class/tty/{tty_name}")).ok()?;
    loop {
        let vid = std::fs::read_to_string(dir.join("idVendor"))
            .ok()
            .and_then(|s| u16::from_str_radix(s.trim(), 16).ok());
        let pid = std::fs::read_to_string(dir.join("idProduct"))
            .ok()
            .and_then(|s| u16::from_str_radix(s.trim(), 16).ok());
        if let (Some(v), Some(p)) = (vid, pid) {
            let manuf = std::fs::read_to_string(dir.join("manufacturer"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            let prod = std::fs::read_to_string(dir.join("product"))
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            return Some((v, p, manuf, prod));
        }
        if !dir.pop() {
            return None;
        }
    }
}
