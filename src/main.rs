use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use log::info;
use std::fs;
use std::path::PathBuf;

mod devices;
mod ports;
mod protocol;
mod tui;
mod updater;

use devices::{find_device, KNOWN_DEVICES};
use protocol::client::{BslClient, FlashType};
use protocol::pac::PacPackage;
use tui::TuiApp;

#[derive(Parser)]
#[command(name = "unisoc-tool")]
#[command(
    about = "⚡ Cross-Platform (Linux & Windows) Unisoc/Spreadtrum Service Tool (BootROM & FDL protocol engine)",
    long_about = None
)]
struct Cli {
    #[cfg_attr(windows, arg(short, long, default_value = "COM3", global = true))]
    #[cfg_attr(not(windows), arg(short, long, default_value = "/dev/ttyUSB0", global = true))]
    port: String,

    #[arg(short, long, default_value_t = 115200, global = true)]
    baud: u32,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the interactive Graphical Terminal User Interface (TUI)
    Tui,

    /// List all supported Realme & Unisoc devices and chipset mappings (from MRST config)
    Devices,

    /// List available serial ports (USB VID/PID when the OS reports them)
    Ports,

    /// Setup Linux udev rules and kernel driver (option) for Unisoc devices
    SetupLinux,

    /// Read Chip ID, Target ID, and Hardware Chip UID
    ReadInfo,

    /// Perform BootROM handshake synchronization (0x7E)
    Handshake {
        #[arg(short, long, default_value_t = 200)]
        attempts: usize,
    },

    /// Send FDL1 and FDL2 to enter full service mode
    Boot {
        /// Chipset or model name (e.g. "T616", "Realme C35", "SC9863A_64")
        #[arg(short, long)]
        model: Option<String>,

        /// Path to custom FDL1 binary
        #[arg(long)]
        fdl1: PathBuf,

        /// Custom FDL1 load address (e.g. 0x00005500)
        #[arg(long)]
        fdl1_addr: Option<String>,

        /// Path to custom FDL2 binary
        #[arg(long)]
        fdl2: PathBuf,

        /// Custom FDL2 load address (e.g. 0x9efffe00)
        #[arg(long)]
        fdl2_addr: Option<String>,

        /// Chunk transfer size in bytes
        #[arg(long, default_value_t = 4096)]
        chunk_size: usize,

        /// EXPERIMENTAL: switch to this baud after FDL2 boots (e.g. 921600).
        /// Payload format is unverified; on failure the session falls back
        /// to the old baud. Omit to stay at --baud.
        #[arg(long)]
        target_baud: Option<u32>,
    },

    /// Inspect Spreadtrum/Unisoc .pac firmware container metadata and partition table
    PacInfo {
        /// Path to .pac firmware file
        #[arg(long)]
        pac: PathBuf,
    },

    /// Unpack all partitions and bootloaders from .pac firmware file to directory
    UnpackPac {
        /// Path to .pac firmware file
        #[arg(long)]
        pac: PathBuf,

        /// Destination directory
        #[arg(short, long)]
        out: PathBuf,
    },

    /// Flash full stock firmware or partitions directly from .pac file to device
    FlashPac {
        /// Path to .pac firmware file
        #[arg(long)]
        pac: PathBuf,

        /// Target device/chipset name (e.g. "Realme C33", "T612") for default FDL addresses
        #[arg(short, long)]
        device: Option<String>,

        /// Comma-separated list of partition IDs to flash (e.g. "boot,recovery,super") or omit for full flash
        #[arg(long, value_delimiter = ',')]
        partitions: Option<Vec<String>>,

        /// Chunk transfer size in bytes (default 16384)
        #[arg(long, default_value_t = 16384)]
        chunk_size: usize,

        /// Storage type of the target phone: auto (flash both SPL variants),
        /// ufs (skip SPLLoaderEMMC), emmc (skip SPLLoaderUFS). The .pac
        /// header field does NOT select this — a UFS phone's .pac reports
        /// flash_type=0 — so pick it yourself when the foreign SPL errors out.
        #[arg(long, default_value = "auto")]
        flash_type: String,

        /// EXPERIMENTAL: switch to this baud after FDL2 boots (e.g. 921600).
        /// Payload format is unverified; on failure the session falls back
        /// to the old baud. Omit to stay at --baud.
        #[arg(long)]
        target_baud: Option<u32>,
    },

    /// Reset Factory Reset Protection (FRP) by wiping FRP partition
    ResetFrp,

    /// Factory Reset: wipe Userdata, Metadata and Cache partitions
    FactoryReset,

    /// Unlock device Bootloader
    UnlockBootloader,

    /// Relock device Bootloader
    RelockBootloader,

    /// Backup / Dump partition to image file
    BackupPartition {
        #[arg(long)]
        partition: String,

        #[arg(short, long)]
        out: PathBuf,

        /// Maximum bytes to dump (optional)
        #[arg(short, long)]
        size: Option<u64>,
    },

    /// Backup NVRAM calibration partitions (prodnv, fixnv1, fixnv2, runtimenv1)
    BackupNvram {
        /// Destination directory for NVRAM backup
        #[arg(short, long, default_value = "./nvram_backup")]
        out: PathBuf,
    },

    /// Restore saved NVRAM calibration partitions to device
    RestoreNvram {
        /// Directory containing saved NVRAM partition dumps
        #[arg(short, long, default_value = "./nvram_backup")]
        dir: PathBuf,
    },

    /// Read live partition table from connected device (requires FDL2)
    Partitions,

    /// Flash / Write image file to partition
    FlashPartition {
        #[arg(long)]
        partition: String,

        #[arg(short, long)]
        file: PathBuf,
    },

    /// Erase specific partition (e.g. "frp", "misc", "userdata")
    ErasePartition {
        #[arg(long)]
        partition: String,
    },

    /// Reboot device back into Android system
    Reboot,

    /// Safely power off device
    PowerOff,

    /// Check for and install software updates from GitHub Releases
    Update {
        /// Force re-download even if already up to date
        #[arg(short, long)]
        force: bool,
    },
}

fn parse_hex_or_dec(s: &str) -> Result<u32> {
    if let Some(stripped) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        u32::from_str_radix(stripped, 16).map_err(|e| anyhow!("Invalid hex: {}", e))
    } else {
        s.parse::<u32>().map_err(|e| anyhow!("Invalid address: {}", e))
    }
}

#[cfg(target_os = "linux")]
fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines().find(|l| l.starts_with("Uid:")).map(|l| {
                let mut parts = l.split_whitespace();
                let _ = parts.next(); // "Uid:"
                let ruid = parts.next();
                let euid = parts.next();
                ruid == Some("0") || euid == Some("0")
            })
        })
        .unwrap_or(false)
}

/// Re-launch this binary as root via sudo (or pkexec) on Linux, forwarding all args.
/// Raw USB (libusb/usbfs) access to VID 1782 needs root on Linux without a udev rule.
/// On Windows and macOS (IOKit), USB access does not use /proc or udev rules.
#[cfg(target_os = "linux")]
fn ensure_root() {
    if is_root() || std::env::var_os("UNISOC_NO_ROOT").is_some() {
        return;
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("unisoc"));
    let args: Vec<String> = std::env::args().skip(1).collect();
    eprintln!("[AUTH] Root required for USB access — restarting via sudo...");
    let have = |b: &str| {
        std::process::Command::new("which").arg(b).output().map(|o| o.status.success()).unwrap_or(false)
    };
    let status = if have("sudo") {
        std::process::Command::new("sudo")
            .arg("--preserve-env=TERM,COLORTERM,LANG,LC_ALL,HOME,PATH")
            .arg("UNISOC_NO_ROOT=1")
            .arg(&exe)
            .args(&args)
            .status()
    } else {
        std::process::Command::new("pkexec")
            .arg("env")
            .arg(format!("TERM={}", std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into())))
            .arg("UNISOC_NO_ROOT=1")
            .arg(&exe)
            .args(&args)
            .status()
    };
    match status {
        Ok(s) => std::process::exit(s.code().unwrap_or(1)),
        Err(e) => eprintln!("[AUTH] Elevation failed ({e}); continuing without root"),
    }
}

#[cfg(not(target_os = "linux"))]
fn ensure_root() {
    // Windows (WinUSB / SPRD U2S Diag) and macOS (IOKit) handle USB access without Linux udev/sudo.
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Offline commands never need root.
    match &cli.command {
        Some(Commands::Devices) | Some(Commands::PacInfo { .. }) | Some(Commands::UnpackPac { .. }) | Some(Commands::Update { .. }) => {}
        _ => ensure_root(),
    }

    // Commands that never touch the port skip resolution; every other
    // command says out loud when it falls back to a detected device.
    match &cli.command {
        None | Some(Commands::Tui) | Some(Commands::Ports) | Some(Commands::Devices) | Some(Commands::SetupLinux) | Some(Commands::Update { .. }) => {}
        _ => {
            if let Some(sprd) = ports::first_unisoc_serial() {
                if cli.port != sprd {
                    eprintln!("[PORT] Detected Unisoc device on {}", sprd);
                }
            }
        }
    }

    match cli.command {
        None | Some(Commands::Tui) => {
            let mut app = TuiApp::new(cli.port, cli.baud);
            app.run()?;
        }

        Some(Commands::Ports) => {
            let found = ports::list_ports();
            if found.is_empty() {
                println!("No serial ports found.");
            } else {
                for p in &found {
                    println!(
                        "{:<16} {}{}",
                        p.name,
                        p.desc,
                        if p.is_sprd() { "  [UNISOC]" } else { "" }
                    );
                }
            }

            let raw = ports::scan_raw_unisoc_devices();
            if !raw.is_empty() {
                println!("\n--- Raw Unisoc USB devices on bus ---");
                for d in raw {
                    if let Some(ref t) = d.tty_name {
                        println!("  Device {}: VID=1782 PID={:04x} -> active port: {}", d.sysfs_name, d.pid, t);
                    } else {
                        println!("  Device {}: VID=1782 PID={:04x} [WARNING: NO SERIAL DRIVER BOUND!]", d.sysfs_name, d.pid);
                        println!("    Fix: run 'unisoc setup-linux' or:");
                        println!("    sudo modprobe option && echo 1782 {:04x} | sudo tee /sys/bus/usb-serial/drivers/option1/new_id", d.pid);
                    }
                }
            }
        }

        Some(Commands::SetupLinux) => {
            let udev_rule = r#"# Spreadtrum / Unisoc BootROM & Download mode (direct libusb + ModemManager ignore)
SUBSYSTEM=="usb", ATTR{idVendor}=="1782", MODE="0666", GROUP="dialout", ENV{ID_MM_DEVICE_IGNORE}="1", ENV{MTP_NO_PROBE}="1"
KERNEL=="ttyUSB*", ATTRS{idVendor}=="1782", MODE="0666", GROUP="dialout", ENV{ID_MM_DEVICE_IGNORE}="1"
KERNEL=="ttyACM*", ATTRS{idVendor}=="1782", MODE="0666", GROUP="dialout", ENV{ID_MM_DEVICE_IGNORE}="1"
"#;
            println!("=========================================================================");
            println!("  UNISOC LINUX SETUP & UDEV RULES HELPER                                ");
            println!("=========================================================================");

            let rule_path = std::path::Path::new("/etc/udev/rules.d/99-unisoc.rules");
            let mut wrote = false;
            if std::fs::write(rule_path, udev_rule).is_ok() {
                println!("[OK] Successfully installed /etc/udev/rules.d/99-unisoc.rules");
                wrote = true;
                let _ = std::process::Command::new("udevadm").args(["control", "--reload-rules"]).output();
                let _ = std::process::Command::new("udevadm").args(["trigger"]).output();
            }

            if !wrote {
                println!("Note: Root permissions are required to install udev rules automatically.");
                println!("\nTo configure your Linux system manually or with sudo, run:\n");
                println!("sudo sh -c 'cat << \"EOF\" > /etc/udev/rules.d/99-unisoc.rules");
                print!("{}", udev_rule);
                println!("EOF'");
                println!("sudo udevadm control --reload-rules && sudo udevadm trigger");
                println!("sudo usermod -aG dialout $USER");
                println!("\n=========================================================================");
            } else {
                println!("Setup complete! Make sure your user is in the 'dialout' group:");
                println!("  sudo usermod -aG dialout $USER");
                println!("=========================================================================");
            }
        }

        Some(Commands::Devices) => {
            println!("=========================================================================");
            println!("  SUPPORTED REALME & UNISOC DEVICES (Parsed from MRST config.ini)       ");
            println!("=========================================================================");
            println!("{:<35} | {:<16} | {:<12} | {:<12}", "Model Name", "Chipset", "FDL1 Addr", "FDL2 Addr");
            println!("-------------------------------------------------------------------------");
            for dev in KNOWN_DEVICES {
                println!(
                    "{:<35} | {:<16} | 0x{:08X}   | 0x{:08X}",
                    dev.name, dev.chip, dev.fdl1_addr, dev.fdl2_addr
                );
            }
            println!("=========================================================================");
        }

        Some(Commands::ReadInfo) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.handshake(100)?;
            client.connect()?;
            let chip = client.read_chip_type()?;
            let uid = client.read_chip_uid()?;
            println!("-----------------------------------------");
            println!("Chip Type: {}", chip);
            println!("Chip UID:  {}", uid);
            println!("-----------------------------------------");
        }

        Some(Commands::Handshake { attempts }) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            info!("Connecting to {} at {} baud...", cli.port, cli.baud);
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.handshake(attempts)?;
            client.connect()?;
            info!("Handshake & Connect successful! Device is ready in BootROM mode.");
        }

        Some(Commands::Boot {
            model,
            fdl1,
            fdl1_addr,
            fdl2,
            fdl2_addr,
            chunk_size,
            target_baud,
        }) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let (target_fdl1_addr, target_fdl2_addr) = if let Some(ref m) = model {
                if let Some(dev) = find_device(m) {
                    info!("Matched device: {} (Chip: {})", dev.name, dev.chip);
                    (dev.fdl1_addr, dev.fdl2_addr)
                } else {
                    return Err(anyhow!("Model '{}' not found in device database. Specify --fdl1-addr and --fdl2-addr manually.", m));
                }
            } else {
                let a1 = fdl1_addr
                    .map(|s| parse_hex_or_dec(&s))
                    .transpose()?
                    .unwrap_or(0x65000800);
                let a2 = fdl2_addr
                    .map(|s| parse_hex_or_dec(&s))
                    .transpose()?
                    .unwrap_or(0x9efffe00);
                (a1, a2)
            };

            let fdl1_bytes = fs::read(&fdl1)
                .map_err(|e| anyhow!("Failed to read FDL1 file '{}': {}", fdl1.display(), e))?;
            let fdl2_bytes = fs::read(&fdl2)
                .map_err(|e| anyhow!("Failed to read FDL2 file '{}': {}", fdl2.display(), e))?;

            info!("Opening serial connection to {}...", cli.port);
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.handshake(200)?;
            client.connect()?;

            client.load_fdl("FDL1", &fdl1_bytes, target_fdl1_addr, chunk_size)?;
            std::thread::sleep(std::time::Duration::from_millis(500));
            client.load_fdl("FDL2", &fdl2_bytes, target_fdl2_addr, chunk_size)?;

            if let Some(b) = target_baud {
                match client.change_baud(b) {
                    Ok(true) => info!("Continuing at {} baud", b),
                    Ok(false) => log::warn!("Continuing at previous baud (switch declined)"),
                    Err(e) => return Err(e),
                }
            }

            info!("FDL2 is active! Device is ready for service operations.");
        }

        Some(Commands::PacInfo { pac }) => {
            let pkg = PacPackage::open(&pac)?;
            println!("===============================================================================");
            println!("  PAC FIRMWARE PACKAGE DETAILS                                                ");
            println!("===============================================================================");
            println!("Product Name:    {}", pkg.product_name);
            println!("Product Version: {}", pkg.product_version);
            println!("Product Alias:   {}", pkg.product_alias);
            println!("Format Version:  {}", pkg.version);
            println!("Total Size:      {} bytes ({:.2} MB)", pkg.total_size, pkg.total_size as f64 / (1024.0 * 1024.0));
            println!("File Count:      {}", pkg.entries.len());
            println!("Header Mode:     {} | Header Flash Type: {} (note: this field does NOT select eMMC/UFS — a UFS phone's .pac reports 0; use --flash-type instead)", pkg.mode, pkg.flash_type);
            if pkg.magic != 0xFFFAFFFA {
                println!("WARNING: unexpected PAC magic 0x{:08X} — file may not be a valid .pac", pkg.magic);
            }
            println!("-------------------------------------------------------------------------------");
            println!("{:<4} | {:<20} | {:<24} | {:<12} | {:<10}", "Idx", "Partition ID", "File Name", "Size (bytes)", "Load Addr");
            println!("-------------------------------------------------------------------------------");
            for (i, entry) in pkg.entries.iter().enumerate() {
                println!(
                    "{:<4} | {:<20} | {:<24} | {:<12} | 0x{:08X}",
                    i + 1,
                    entry.file_id,
                    entry.file_name,
                    entry.size,
                    entry.address
                );
            }
            println!("===============================================================================");
        }

        Some(Commands::UnpackPac { pac, out }) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let pkg = PacPackage::open(&pac)?;
            pkg.unpack_all(&pac, &out)?;
            println!("Successfully unpacked PAC firmware into '{}'.", out.display());
        }

        Some(Commands::FlashPac {
            pac,
            device,
            partitions,
            chunk_size,
            flash_type,
            target_baud,
        }) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let flash_type = FlashType::parse(&flash_type)?;
            let (fdl1_addr, fdl2_addr) = if let Some(ref d) = device {
                if let Some(dev) = find_device(d) {
                    info!("Matched device: {} (Chip: {})", dev.name, dev.chip);
                    (dev.fdl1_addr, dev.fdl2_addr)
                } else {
                    info!("Device '{}' not in database, using default Transsion/T606 addresses", d);
                    (0x65000800, 0x9efffe00)
                }
            } else {
                (0x65000800, 0x9efffe00)
            };

            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.flash_pac(
                &pac,
                fdl1_addr,
                fdl2_addr,
                chunk_size,
                partitions.as_deref(),
                flash_type,
                target_baud,
            )?;
        }

        Some(Commands::ResetFrp) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.reset_frp()?;
        }

        Some(Commands::FactoryReset) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.factory_reset()?;
        }

        Some(Commands::UnlockBootloader) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.unlock_bootloader()?;
        }

        Some(Commands::RelockBootloader) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.relock_bootloader()?;
        }

        Some(Commands::BackupPartition { partition, out, size }) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.backup_partition(&partition, &out, size)?;
        }

        Some(Commands::BackupNvram { out }) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.backup_nvram(&out)?;
        }

        Some(Commands::RestoreNvram { dir }) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.restore_nvram(&dir)?;
        }

        Some(Commands::Partitions) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            let list = client.read_partition_table()?;
            println!("===============================================================================");
            println!("  DEVICE LIVE PARTITIONS (Read from FDL2)                                      ");
            println!("===============================================================================");
            println!("{:<4} | {:<28} | {:<16}", "Idx", "Partition Name", "Size (bytes)");
            println!("-------------------------------------------------------------------------------");
            for (i, (name, size)) in list.iter().enumerate() {
                println!("{:<4} | {:<28} | {:<16}", i + 1, name, size);
            }
            println!("===============================================================================");
        }

        Some(Commands::FlashPartition { partition, file }) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.flash_partition(&partition, &file)?;
        }

        Some(Commands::ErasePartition { partition }) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.erase_partition(&partition)?;
        }

        Some(Commands::Reboot) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.reboot()?;
        }

        Some(Commands::PowerOff) => {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
            let mut client = BslClient::wait_and_open(&cli.port, cli.baud, std::time::Duration::from_secs(30))?;
            client.power_off()?;
        }

        Some(Commands::Update { force }) => {
            updater::run_self_update(force)?;
        }
    }

    Ok(())
}
