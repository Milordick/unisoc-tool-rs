use anyhow::{anyhow, Context, Result};
use byteorder::{BigEndian, ByteOrder, LittleEndian};
use indicatif::{ProgressBar, ProgressStyle};
use log::{debug, info, warn};
use serialport::SerialPort;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::Duration;

use super::constants::*;
use super::frame::{decode_frame, encode_frame};
use super::pac::PacPackage;

pub enum Transport {
    Serial(Box<dyn SerialPort>),
    Usb(super::usb_transport::UsbTransport),
}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Transport::Serial(s) => s.read(buf),
            Transport::Usb(u) => u.read(buf),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Transport::Serial(s) => s.write(buf),
            Transport::Usb(u) => u.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Transport::Serial(s) => s.flush(),
            Transport::Usb(u) => u.flush(),
        }
    }
}

impl Transport {
    pub fn set_timeout(&mut self, timeout: Duration) -> Result<()> {
        match self {
            Transport::Serial(s) => s.set_timeout(timeout).map_err(Into::into),
            Transport::Usb(u) => {
                u.set_timeout(timeout);
                Ok(())
            }
        }
    }

    pub fn timeout(&self) -> Duration {
        match self {
            Transport::Serial(s) => s.timeout(),
            Transport::Usb(u) => u.timeout(),
        }
    }

    pub fn set_baud_rate(&mut self, baud: u32) -> Result<()> {
        match self {
            Transport::Serial(s) => s.set_baud_rate(baud).map_err(Into::into),
            Transport::Usb(_) => Ok(()),
        }
    }
}

pub struct BslClient {
    transport: Transport,
    pub use_crc: bool,
    baud_rate: u32,
    strict_checksum: bool,
}

/// Storage type selector for `flash-pac`.
///
/// The .pac itself cannot decide this: a stock .pac carries BOTH
/// `SPLLoaderUFS` and `SPLLoaderEMMC` with identical flags, and its header
/// `flash_type` field is NOT a storage selector (a UFS phone's V152 .pac
/// reports flash_type=0). The decision has to come from the user (this flag)
/// or from the device. `Auto` flashes both SPL variants and relies on
/// FDL2 accepting the names; use `Ufs`/`Emmc` when the foreign SPL entry
/// errors out on your phone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlashType {
    #[default]
    Auto,
    Ufs,
    Emmc,
}

impl FlashType {
    pub fn parse(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(FlashType::Auto),
            "ufs" => Ok(FlashType::Ufs),
            "emmc" => Ok(FlashType::Emmc),
            other => Err(anyhow!("Unknown flash type '{}' (expected auto, ufs or emmc)", other)),
        }
    }

    /// True when this entry is built for the OTHER storage type and must be
    /// skipped. Only the SPL loader names carry a storage marker; every
    /// other entry is storage-agnostic.
    fn skips(&self, file_id: &str) -> bool {
        let upper = file_id.to_ascii_uppercase();
        match self {
            FlashType::Auto => false,
            FlashType::Ufs => upper.contains("EMMC"),
            FlashType::Emmc => upper.contains("UFS"),
        }
    }
}

impl BslClient {
    pub fn auto_detect_port() -> Option<String> {
        // A real Unisoc device beats any other USB-serial gadget.
        // Never let a non-Unisoc device (e.g. Raspberry Pi Pico) steal detection.
        crate::ports::first_unisoc_serial()
    }

    /// True if `port_name` exists as a device file (Unix) or in the OS serial port list (Windows/Unix).
    fn port_is_present(port_name: &str) -> bool {
        if Path::new(port_name).exists() {
            return true;
        }
        crate::ports::list_ports()
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(port_name))
    }

    /// The port a command will actually use: the requested one if it
    /// exists, else the auto-detected device, else the requested one
    /// (so the error names a real path). Callers log the switch —
    /// open() used to do this silently, which made "title says USB0,
    /// error says ACM1" look like a bug.
    pub fn resolve_port(port_name: &str) -> String {
        if Self::port_is_present(port_name) {
            return port_name.to_string();
        }
        Self::auto_detect_port().unwrap_or_else(|| port_name.to_string())
    }

    pub fn open_serial(port_name: &str, baud_rate: u32) -> Result<Self> {
        let actual_port = Self::resolve_port(port_name);

        let port = serialport::new(&actual_port, baud_rate)
            .timeout(Duration::from_millis(2000))
            .open()
            .with_context(|| format!("Failed to open serial port {}", actual_port))?;

        Ok(Self {
            transport: Transport::Serial(port),
            use_crc: true,
            baud_rate,
            strict_checksum: std::env::var("UNISOC_STRICT_CHECKSUM")
                .map(|v| v != "0")
                .unwrap_or(false),
        })
    }

    #[allow(dead_code)]
    pub fn open(port_name: &str, baud_rate: u32) -> Result<Self> {
        Self::wait_and_open(port_name, baud_rate, Duration::from_millis(500))
    }

    /// Wait for a Unisoc device to appear in BootROM mode, then open it.
    /// Prefers direct USB connection (like spd_dump), and falls back to serial.
    pub fn wait_and_open(port_name: &str, baud_rate: u32, timeout: Duration) -> Result<Self> {
        let is_default_port = port_name == "/dev/ttyUSB0" || port_name.eq_ignore_ascii_case("COM3");
        let is_custom_port = !is_default_port
            && !port_name.starts_with("USB:")
            && Self::port_is_present(port_name);
        if is_custom_port {
            return Self::open_serial(port_name, baud_rate);
        }

        info!("⌛ Waiting for Unisoc device in BootROM mode (turn phone OFF, hold Vol- or Vol+, plug USB)...");
        let start = std::time::Instant::now();

        while start.elapsed() < timeout {
            // 1. First priority: Direct USB bulk connection (identical to spd_dump)
            if let Ok(usb) = super::usb_transport::UsbTransport::open_unisoc(Duration::from_millis(200)) {
                info!("⚡ Connected directly to Unisoc device via USB (raw bulk endpoints)!");
                return Ok(Self {
                    transport: Transport::Usb(usb),
                    use_crc: true,
                    baud_rate,
                    strict_checksum: std::env::var("UNISOC_STRICT_CHECKSUM")
                        .map(|v| v != "0")
                        .unwrap_or(false),
                });
            }

            // 2. Second priority: Active Unisoc serial port (e.g. Windows SPRD U2S Diag COMx or Linux ttyUSBx)
            if let Some(p) = crate::ports::first_unisoc_serial() {
                info!("⚡ Detected Unisoc device on serial port {}! Connecting...", p);
                return Self::open_serial(&p, baud_rate);
            }

            // 3. Third priority: Specific custom port appeared
            if !is_default_port && !port_name.starts_with("USB:") && Self::port_is_present(port_name) {
                return Self::open_serial(port_name, baud_rate);
            }

            std::thread::sleep(Duration::from_millis(150));
        }

        // Final attempt via direct USB or serial
        if let Ok(usb) = super::usb_transport::UsbTransport::open_unisoc(Duration::from_millis(500)) {
            info!("⚡ Connected directly to Unisoc device via USB (raw bulk endpoints)!");
            return Ok(Self {
                transport: Transport::Usb(usb),
                use_crc: true,
                baud_rate,
                strict_checksum: std::env::var("UNISOC_STRICT_CHECKSUM")
                    .map(|v| v != "0")
                    .unwrap_or(false),
            });
        }

        Self::open_serial(port_name, baud_rate)
    }

    pub fn write_raw(&mut self, data: &[u8]) -> Result<()> {
        self.transport.write_all(data)?;
        self.transport.flush()?;
        Ok(())
    }

    pub fn read_frame(&mut self) -> Result<(u16, Vec<u8>)> {
        // Buffered reads instead of byte-by-byte: a 16 KiB READ_MIDST payload
        // used to cost one syscall per byte. We still scan for the HDLC flags
        // in order, so framing semantics are unchanged.
        let mut buffer = Vec::new();
        let mut started = false;
        let mut chunk = [0u8; 4096];

        for _ in 0..131072 {
            match self.transport.read(&mut chunk) {
                Ok(0) => {
                    return Err(anyhow!("Read frame timed out"));
                }
                Ok(n) => {
                    for &b in &chunk[..n] {
                        if b == HDLC_FLAG {
                            if !started {
                                started = true;
                                buffer.push(HDLC_FLAG);
                            } else if buffer.len() == 1 {
                                // Consecutive 0x7E delimiter; treat as fresh frame start
                                continue;
                            } else {
                                buffer.push(HDLC_FLAG);
                                return decode_frame(&buffer, self.use_crc, self.strict_checksum);
                            }
                        } else if started {
                            buffer.push(b);
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                    return Err(anyhow!("Read frame timed out"));
                }
                Err(e) => return Err(e.into()),
            }
        }

        Err(anyhow!("Empty or incomplete frame read"))
    }

    pub fn send_command(&mut self, cmd: u16, payload: &[u8]) -> Result<(u16, Vec<u8>)> {
        let frame = encode_frame(cmd, payload, self.use_crc);
        debug!("Sending frame cmd 0x{:04X}, raw length: {}", cmd, frame.len());
        self.write_raw(&frame)?;
        self.read_frame()
    }

    pub fn handshake(&mut self, max_attempts: usize) -> Result<()> {
        info!("Sending handshake (0x7E synchronization)...");
        let sync_byte = [HDLC_FLAG];
        let mut buf = [0u8; 1];

        // Lower timeout during handshake so failed reads don't block for 2000ms each
        let orig_timeout = self.transport.timeout();
        let _ = self.transport.set_timeout(Duration::from_millis(30));
        let _ = self.drain_port();

        for attempt in 1..=max_attempts {
            if let Err(e) = self.transport.write_all(&sync_byte) {
                let _ = self.transport.set_timeout(orig_timeout);
                return Err(e.into());
            }
            let _ = self.transport.flush();

            std::thread::sleep(Duration::from_millis(15));

            if self.transport.read(&mut buf).is_ok() && buf[0] == HDLC_FLAG {
                info!("Device synchronized on attempt {}!", attempt);
                let _ = self.drain_port();
                let _ = self.transport.set_timeout(orig_timeout);
                return Ok(());
            }
        }

        let _ = self.transport.set_timeout(orig_timeout);
        Err(anyhow!(
            "Device did not respond to handshake after {} attempts. Check USB cable and Boot Key.",
            max_attempts
        ))
    }

    pub fn connect(&mut self) -> Result<()> {
        info!("Sending BSL_CMD_CONNECT (0x00)...");
        let (rep, _) = match self.send_command(BSL_CMD_CONNECT, &[]) {
            Ok(r) if r.0 == BSL_REP_ACK => r,
            _ => {
                // Toggle CRC mode (BootROM uses CRC16, FDL1/FDL2 uses SPD checksum) and retry
                self.use_crc = !self.use_crc;
                self.send_command(BSL_CMD_CONNECT, &[])?
            }
        };
        if rep == BSL_REP_ACK {
            info!("Connected to Unisoc Bootloader/ROM!");
            Ok(())
        } else {
            Err(anyhow!("Connect failed: {}", response_name(rep)))
        }
    }

    pub fn read_chip_type(&mut self) -> Result<String> {
        info!("Reading chip type (BSL_CMD_READ_CHIP_TYPE)...");
        let (rep, payload) = self.send_command(BSL_CMD_READ_CHIP_TYPE, &[])?;
        if rep == BSL_REP_ACK || rep == BSL_REP_VER || rep == BSL_REP_READ_CHIP_TYPE {
            let info_str = String::from_utf8_lossy(&payload).trim().to_string();
            info!("Chip Type: {}", info_str);
            Ok(info_str)
        } else {
            Err(anyhow!("Failed to read chip type: {}", response_name(rep)))
        }
    }

    pub fn read_chip_uid(&mut self) -> Result<String> {
        info!("Reading unique Chip UID (BSL_CMD_READ_CHIP_UID)...");
        let (rep, payload) = self.send_command(BSL_CMD_READ_CHIP_UID, &[])?;
        if rep == BSL_REP_ACK || rep == BSL_REP_READ_CHIP_UID {
            let uid_hex = hex_dump(&payload);
            info!("Chip UID: {}", uid_hex);
            Ok(uid_hex)
        } else {
            Err(anyhow!("Failed to read chip UID: {}", response_name(rep)))
        }
    }

    pub fn read_partition_table(&mut self) -> Result<Vec<(String, u64)>> {
        info!("Querying device partition table (BSL_CMD_READ_PARTITION)...");
        let (rep, payload) = self.send_command(BSL_CMD_READ_PARTITION, &[])?;
        if rep != BSL_REP_READ_PARTITION && rep != BSL_REP_ACK {
            return Err(anyhow!(
                "Device does not support partition query in this state: {}",
                response_name(rep)
            ));
        }

        let entry_size = 76;
        let count = payload.len() / entry_size;
        let mut partitions = Vec::with_capacity(count);

        for i in 0..count {
            let chunk = &payload[i * entry_size..(i + 1) * entry_size];
            let name_raw = &chunk[0..72];
            let size = LittleEndian::read_u32(&chunk[72..76]) as u64;

            let mut name = String::new();
            for j in (0..72).step_by(2) {
                let u = LittleEndian::read_u16(&name_raw[j..j + 2]);
                if u == 0 {
                    break;
                }
                if let Some(c) = char::from_u32(u as u32) {
                    name.push(c);
                }
            }
            if !name.is_empty() {
                partitions.push((name, size));
            }
        }
        Ok(partitions)
    }

    pub fn load_fdl(
        &mut self,
        fdl_name: &str,
        fdl_data: &[u8],
        base_address: u32,
        chunk_size: usize,
    ) -> Result<()> {
        info!(
            "Loading {} ({} bytes) to address 0x{:08X}...",
            fdl_name,
            fdl_data.len(),
            base_address
        );

        let mut start_payload = [0u8; 8];
        BigEndian::write_u32(&mut start_payload[0..4], base_address);
        BigEndian::write_u32(&mut start_payload[4..8], fdl_data.len() as u32);

        let (rep, _) = self.send_command(BSL_CMD_START_DATA, &start_payload)?;
        if rep != BSL_REP_ACK {
            return Err(anyhow!("BSL_CMD_START_DATA failed: {}", response_name(rep)));
        }

        let pb = ProgressBar::new(fdl_data.len() as u64);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{msg} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap(),
        );
        pb.set_message(format!("Uploading {}", fdl_name));

        for chunk in fdl_data.chunks(chunk_size) {
            let (rep, _) = self.send_command(BSL_CMD_MIDST_DATA, chunk)?;
            if rep != BSL_REP_ACK {
                return Err(anyhow!("BSL_CMD_MIDST_DATA failed: {}", response_name(rep)));
            }
            pb.inc(chunk.len() as u64);
        }
        pb.finish_with_message(format!("Uploaded {}", fdl_name));

        let (rep, _) = self.send_command(BSL_CMD_END_DATA, &[])?;
        if rep != BSL_REP_ACK {
            return Err(anyhow!("BSL_CMD_END_DATA failed: {}", response_name(rep)));
        }

        info!("Executing {} at 0x{:08X}...", fdl_name, base_address);
        let mut exec_payload = [0u8; 4];
        BigEndian::write_u32(&mut exec_payload, base_address);
        let (rep, _) = self.send_command(BSL_CMD_EXEC_DATA, &exec_payload)?;
        if rep != BSL_REP_ACK {
            return Err(anyhow!("BSL_CMD_EXEC_DATA failed: {}", response_name(rep)));
        }

        info!("{} executed successfully!", fdl_name);
        if fdl_name.eq_ignore_ascii_case("FDL1") {
            // FDL1 uses Spreadtrum ones-complement checksum and requires 0x7E + CONNECT
            self.use_crc = false;
            std::thread::sleep(Duration::from_millis(250));
            let _ = self.handshake(100);
            let _ = self.connect();
        }
        Ok(())
    }

    pub fn erase_partition(&mut self, partition_name: &str) -> Result<()> {
        info!("Erasing partition '{}'...", partition_name);
        // Modern FDL2 packet: 36 UTF-16LE characters (72 bytes) + 4 bytes LE size (0)
        let mut pkt = vec![0u8; 76];
        let utf16: Vec<u16> = partition_name.encode_utf16().take(35).collect();
        for (i, &u) in utf16.iter().enumerate() {
            pkt[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }

        let (rep, _) = self.send_command(BSL_CMD_ERASE_FLASH, &pkt)?;
        if rep == BSL_REP_ACK {
            info!("Partition '{}' erased successfully!", partition_name);
            return Ok(());
        }

        // Fallback to null-terminated ASCII string (for legacy BSL / feature phones)
        let mut name_bytes = partition_name.as_bytes().to_vec();
        name_bytes.push(0);
        let (rep2, _) = self.send_command(BSL_CMD_ERASE_FLASH, &name_bytes)?;
        if rep2 == BSL_REP_ACK {
            info!("Partition '{}' erased successfully (legacy mode)!", partition_name);
            Ok(())
        } else {
            Err(anyhow!(
                "Failed to erase partition '{}': {}",
                partition_name,
                response_name(rep)
            ))
        }
    }

    pub fn reset_frp(&mut self) -> Result<()> {
        info!("Executing Factory Reset Protection (FRP) wipe...");
        // 'frp' must succeed — a failed wipe must never be reported as success.
        // 'persistent'/'config' are best-effort: warned about, not fatal.
        self.erase_partition("frp")?;
        if let Err(e) = self.erase_partition("persistent") {
            warn!("Could not erase 'persistent' (non-fatal): {}", e);
        }
        if let Err(e) = self.erase_partition("config") {
            warn!("Could not erase 'config' (non-fatal): {}", e);
        }
        info!("FRP Reset completed.");
        Ok(())
    }

    pub fn factory_reset(&mut self) -> Result<()> {
        info!("Executing Factory Reset (Wiping Userdata & Metadata)...");
        self.erase_partition("userdata")?;
        if let Err(e) = self.erase_partition("metadata") {
            warn!("Could not erase 'metadata' (non-fatal): {}", e);
        }
        if let Err(e) = self.erase_partition("cache") {
            warn!("Could not erase 'cache' (non-fatal): {}", e);
        }
        info!("Factory Reset completed successfully.");
        Ok(())
    }

    pub fn unlock_bootloader(&mut self) -> Result<()> {
        // Deliberate refusal: erasing 'misc'/'metadata' does NOT unlock the
        // bootloader on modern secure-boot Unisoc devices. It only wipes
        // state and can break recovery/fastboot, while the user walks away
        // believing the device is unlocked. Real unlock paths are
        // device-specific (OEM unlocking toggle + a vendor/CVE exploit where
        // applicable, e.g. CVE-2022-38694 on some Tiger T606 phones).
        // Nothing is written to the device here.
        Err(anyhow!(
            "Refusing: erasing 'misc'/'metadata' is not a bootloader unlock on modern secure-boot Unisoc devices. \
             Use the device-specific unlock path (OEM unlocking toggle + exploit/firmware flow for your model) instead. \
             See README for details."
        ))
    }

    pub fn relock_bootloader(&mut self) -> Result<()> {
        // Same reasoning as unlock_bootloader: wiping 'misc' does not relock
        // anything on secure-boot devices, it only destroys state.
        Err(anyhow!(
            "Refusing: erasing 'misc' is not a bootloader relock on modern secure-boot Unisoc devices. \
             Relocking is a device-specific signed operation, not a partition wipe. See README for details."
        ))
    }

    pub fn backup_partition(
        &mut self,
        partition_name: &str,
        output_path: &Path,
        length: Option<u64>,
    ) -> Result<()> {
        // Size: explicit --size wins; otherwise ask the device for the real
        // partition size from its table; only as a last resort fall back to
        // 64MB (which silently truncates anything larger).
        let dump_len = match length {
            Some(n) => n,
            None => match self.read_partition_table() {
                Ok(table) => match table.iter().find(|(name, _)| name == partition_name) {
                    Some((_, size)) => {
                        info!("Partition '{}' size from device table: {} bytes", partition_name, size);
                        *size
                    }
                    None => {
                        warn!("Partition '{}' not in device table; falling back to 64MB default", partition_name);
                        67108864
                    }
                },
                Err(e) => {
                    warn!("Could not read partition table ({}); falling back to 64MB default", e);
                    67108864
                }
            },
        };
        info!("Reading partition '{}' (up to {} bytes) into {:?}...", partition_name, dump_len, output_path);

        // Send READ_START with UTF-16LE partition header
        let is_64bit = dump_len > 0xFFFFFFFF;
        let mut start_pkt = vec![0u8; if is_64bit { 88 } else { 76 }];
        let utf16: Vec<u16> = partition_name.encode_utf16().take(35).collect();
        for (i, &u) in utf16.iter().enumerate() {
            start_pkt[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        LittleEndian::write_u32(&mut start_pkt[72..76], dump_len as u32);
        if is_64bit {
            LittleEndian::write_u32(&mut start_pkt[76..80], (dump_len >> 32) as u32);
        }

        let (rep, _) = self.send_command(BSL_CMD_READ_START, &start_pkt)?;
        if rep != BSL_REP_ACK {
            return Err(anyhow!("BSL_CMD_READ_START failed for '{}': {}", partition_name, response_name(rep)));
        }

        let mut out_file = File::create(output_path)?;
        let step = 16384u32;
        let mut offset = 0u64;
        let pb = ProgressBar::new(dump_len);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{msg} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap(),
        );
        pb.set_message(format!("Dumping {}", partition_name));

        while offset < dump_len {
            let to_read = std::cmp::min(step as u64, dump_len - offset) as u32;
            let mut read_pkt = vec![0u8; if is_64bit { 12 } else { 8 }];
            LittleEndian::write_u32(&mut read_pkt[0..4], to_read);
            LittleEndian::write_u32(&mut read_pkt[4..8], offset as u32);
            if is_64bit {
                LittleEndian::write_u32(&mut read_pkt[8..12], (offset >> 32) as u32);
            }

            let (rep, payload) = self.send_command(BSL_CMD_READ_MIDST, &read_pkt)?;
            if rep != BSL_REP_READ_FLASH && rep != BSL_REP_ACK {
                break;
            }

            // Payload format: 2-byte big-endian length + chunk bytes
            let chunk_data = if payload.len() >= 2 {
                let nread = BigEndian::read_u16(&payload[0..2]) as usize;
                if payload.len() >= 2 + nread {
                    &payload[2..2 + nread]
                } else {
                    &payload[2..]
                }
            } else {
                &payload[..]
            };

            if chunk_data.is_empty() {
                break;
            }

            out_file.write_all(chunk_data)?;
            offset += chunk_data.len() as u64;
            pb.inc(chunk_data.len() as u64);
        }

        pb.finish_with_message(format!("Dumped {}", partition_name));
        let _ = self.send_command(BSL_CMD_READ_END, &[]);
        info!("Saved {} bytes for partition '{}'", offset, partition_name);
        Ok(())
    }

    pub fn backup_nvram(&mut self, output_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(output_dir)?;
        info!("Backing up NVRAM calibration partitions to {:?}...", output_dir);
        let nv_parts = ["prodnv", "fixnv1", "fixnv2", "runtimenv1"];
        for part in nv_parts {
            let out_file = output_dir.join(format!("{}.bin", part));
            info!("Dumping NV partition '{}' -> {:?}", part, out_file);
            let _ = self.backup_partition(part, &out_file, Some(8388608));
        }
        info!("NVRAM backup completed successfully.");
        Ok(())
    }

    pub fn restore_nvram(&mut self, input_dir: &Path) -> Result<()> {
        info!("Restoring NVRAM partitions from {:?}...", input_dir);
        let nv_parts = ["prodnv", "fixnv1", "fixnv2", "runtimenv1"];
        for part in nv_parts {
            let in_file = input_dir.join(format!("{}.bin", part));
            if in_file.exists() {
                info!("Flashing NV partition '{}' from {:?}", part, in_file);
                self.flash_partition(part, &in_file)?;
            }
        }
        info!("NVRAM restore completed successfully.");
        Ok(())
    }

    pub fn flash_partition_stream<R: Read>(
        &mut self,
        partition_name: &str,
        mut reader: R,
        total_size: u64,
        chunk_size: usize,
    ) -> Result<()> {
        info!("Flashing partition '{}' ({} bytes)...", partition_name, total_size);

        // Send START_DATA with 76/88 byte header (36 UTF-16LE characters + size)
        let is_64bit = total_size > 0xFFFFFFFF;
        let mut start_pkt = vec![0u8; if is_64bit { 88 } else { 76 }];
        let utf16: Vec<u16> = partition_name.encode_utf16().take(35).collect();
        for (i, &u) in utf16.iter().enumerate() {
            start_pkt[i * 2..i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
        LittleEndian::write_u32(&mut start_pkt[72..76], total_size as u32);
        if is_64bit {
            LittleEndian::write_u32(&mut start_pkt[76..80], (total_size >> 32) as u32);
        }

        let (rep, _) = self.send_command(BSL_CMD_START_DATA, &start_pkt)?;
        if rep != BSL_REP_ACK {
            return Err(anyhow!(
                "BSL_CMD_START_DATA rejected for '{}': {}",
                partition_name,
                response_name(rep)
            ));
        }

        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{msg} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap(),
        );
        pb.set_message(format!("Flashing {}", partition_name));

        let mut buffer = vec![0u8; chunk_size];
        let mut remaining = total_size;
        while remaining > 0 {
            let to_read = std::cmp::min(remaining, chunk_size as u64) as usize;
            reader.read_exact(&mut buffer[..to_read])?;
            let (rep, _) = self.send_command(BSL_CMD_MIDST_DATA, &buffer[..to_read])?;
            if rep != BSL_REP_ACK {
                return Err(anyhow!(
                    "BSL_CMD_MIDST_DATA failed for '{}': {}",
                    partition_name,
                    response_name(rep)
                ));
            }
            pb.inc(to_read as u64);
            remaining -= to_read as u64;
        }
        pb.finish_with_message(format!("Flashed {}", partition_name));

        let (rep, _) = self.send_command(BSL_CMD_END_DATA, &[])?;
        if rep != BSL_REP_ACK {
            return Err(anyhow!(
                "BSL_CMD_END_DATA failed for '{}': {}",
                partition_name,
                response_name(rep)
            ));
        }

        info!("Partition '{}' flashed successfully!", partition_name);
        Ok(())
    }

    pub fn flash_partition(&mut self, partition_name: &str, file_path: &Path) -> Result<()> {
        let file = File::open(file_path)?;
        let metadata = file.metadata()?;
        let total_size = metadata.len();
        self.flash_partition_stream(partition_name, file, total_size, 16384)
    }

    /// Boot the device into FDL2 mode from a .pac package: handshake, load
    /// FDL1 then FDL2, and STOP — no flashing, no reboot. This is the path
    /// the TUI "Boot" action and the CLI `boot` command use when the goal is
    /// to keep the phone in FDL2 mode for manual partition work.
    /// Returns the opened .pac package and file handle for further use.
    pub fn boot_from_pac(
        &mut self,
        pac_path: &Path,
        default_fdl1_addr: u32,
        default_fdl2_addr: u32,
        chunk_size: usize,
    ) -> Result<(PacPackage, File)> {
        info!("Loading PAC firmware package: {:?}", pac_path);
        let pac = PacPackage::open(pac_path)?;
        info!("PAC Product: {}", pac.product_name);
        info!("PAC Version: {}", pac.product_version);
        info!("PAC File Count: {}", pac.entries.len());

        let mut pac_file = File::open(pac_path)?;

        let fdl1_entry = pac
            .find_fdl1()
            .ok_or_else(|| anyhow!("PAC package does not contain FDL1 bootloader"))?
            .clone();
        let fdl2_entry = pac
            .find_fdl2()
            .ok_or_else(|| anyhow!("PAC package does not contain FDL2 bootloader"))?
            .clone();

        let fdl1_addr = if fdl1_entry.address != 0 && fdl1_entry.address < 0xE0000000 {
            fdl1_entry.address
        } else {
            default_fdl1_addr
        };

        let fdl2_addr = if fdl2_entry.address != 0 && fdl2_entry.address < 0xE0000000 {
            fdl2_entry.address
        } else {
            default_fdl2_addr
        };

        info!("Extracting FDL1 (addr: 0x{:08X})...", fdl1_addr);
        let fdl1_bytes = pac.read_entry_bytes(&mut pac_file, &fdl1_entry)?;
        info!("Extracting FDL2 (addr: 0x{:08X})...", fdl2_addr);
        let fdl2_bytes = pac.read_entry_bytes(&mut pac_file, &fdl2_entry)?;

        info!("Performing BootROM handshake synchronization...");
        self.handshake(200)?;
        self.connect()?;

        info!("Booting FDL1 ({} bytes)...", fdl1_bytes.len());
        self.load_fdl("FDL1", &fdl1_bytes, fdl1_addr, 2048)?;
        std::thread::sleep(Duration::from_millis(500));

        info!("Booting FDL2 ({} bytes)...", fdl2_bytes.len());
        self.load_fdl("FDL2", &fdl2_bytes, fdl2_addr, chunk_size)?;
        std::thread::sleep(Duration::from_millis(500));
        self.reconnect_if_needed(Duration::from_secs(5))?;

        info!("FDL2 active! Device is in download mode.");
        Ok((pac, pac_file))
    }

    pub fn flash_pac(
        &mut self,
        pac_path: &Path,
        default_fdl1_addr: u32,
        default_fdl2_addr: u32,
        chunk_size: usize,
        partitions_filter: Option<&[String]>,
        flash_type: FlashType,
        target_baud: Option<u32>,
    ) -> Result<()> {
        let (pac, mut pac_file) =
            self.boot_from_pac(pac_path, default_fdl1_addr, default_fdl2_addr, chunk_size)?;

        // Optional baud switch, applied only after FDL2 is active and only
        // when the user asked for it (--target-baud). A failed switch falls
        // back to the old baud; a lost session aborts the flash.
        if let Some(b) = target_baud {
            match self.change_baud(b) {
                Ok(true) => info!("Continuing flash at {} baud", b),
                Ok(false) => warn!("Continuing flash at {} baud (switch declined)", self.baud_rate),
                Err(e) => return Err(e),
            }
        }

        // NOTE: no repartition step here. flash_pac assumes the device already
        // has a compatible partition layout (same-firmware reflash). Changing
        // the layout needs a BSL_CMD_REPARTITION flow that this tool does not
        // implement, for two verified reasons: (1) the .pac carries no layout
        // table — its trailing entry, despite the "X669D.xml" name, is a PNG
        // asset, not XML; (2) the 0x0B wire format is not public, and sending
        // a guessed layout could shrink partitions (e.g. userdata to its
        // 2.9 MB image size) and brick the phone. Use MRST/ResearchDownload
        // for layout changes. The path to implement it is a USB capture of
        // MRST performing a layout-changing flash.
        let mut failures: Vec<String> = Vec::new();

        for (idx, entry) in pac.entries.iter().enumerate() {
            // Flag bit 0 = "download this entry". Verified across all 43
            // entries of a stock .pac: real partitions and FDLs have bit 0
            // set (FDLs: 257), erase markers have flag 0, and the trailing
            // asset has flag 2 (bit 0 clear). The old `flag == 0` check let
            // the asset through; the bit test excludes it properly.
            if entry.size == 0 || (entry.flag & 1) == 0 || entry.file_id.is_empty() || entry.is_fdl1() || entry.is_fdl2() {
                continue;
            }

            // Storage-type selection: skip the SPL loader built for the
            // other storage type (e.g. SPLLoaderEMMC on a UFS phone).
            if flash_type.skips(&entry.file_id) {
                info!("Skipping {} (built for the other storage type, target is {:?})", entry.file_id, flash_type);
                continue;
            }

            if let Some(filter) = partitions_filter {
                let matches = filter.iter().any(|f| {
                    f.eq_ignore_ascii_case(&entry.file_id) || f.eq_ignore_ascii_case(&entry.file_name)
                });
                if !matches {
                    continue;
                }
            }

            info!(
                "[{}/{}] Flashing partition: {} ({} bytes)...",
                idx + 1,
                pac.entries.len(),
                entry.file_id,
                entry.size
            );

            let result = (|| -> Result<()> {
                pac_file.seek(SeekFrom::Start(entry.offset))?;
                let take_reader = (&mut pac_file).take(entry.size);
                self.flash_partition_stream(&entry.file_id, take_reader, entry.size, chunk_size)
            })();

            // One bad entry (e.g. an eMMC-only loader on a UFS phone) must not
            // abort the rest of the flash, but it must not be hidden either.
            if let Err(e) = result {
                warn!("Failed to flash '{}': {} — continuing with remaining partitions", entry.file_id, e);
                failures.push(format!("{}: {}", entry.file_id, e));
            }
        }

        if !failures.is_empty() {
            return Err(anyhow!(
                "Flash finished with {} failed partition(s), device NOT rebooted:\n  {}",
                failures.len(),
                failures.join("\n  ")
            ));
        }

        info!("All partitions flashed successfully! Rebooting device...");
        let _ = self.reboot();
        Ok(())
    }

    /// EXPERIMENTAL, opt-in only: switch the session to a higher baud rate
    /// (e.g. 921600) after FDL2 is active, to speed up large transfers.
    ///
    /// Payload format is NOT confirmed from public sources: this sends the
    /// target baud as a big-endian u32, the convention this protocol family
    /// uses for control values. Returns Ok(true) if the switch verified,
    /// Ok(false) if the device declined or ignored the command (the session
    /// continues at the old baud), Err only if the session itself is lost.
    pub fn change_baud(&mut self, new_baud: u32) -> Result<bool> {
        let old_baud = self.baud_rate;
        if new_baud == old_baud {
            return Ok(true);
        }
        info!("Requesting baud switch {} -> {} ...", old_baud, new_baud);
        let mut payload = [0u8; 4];
        BigEndian::write_u32(&mut payload, new_baud);
        match self.send_command(BSL_CMD_CHANGE_BAUD, &payload) {
            Ok((rep, _)) if rep == BSL_REP_ACK => {
                info!("Device ACKed baud switch, reconfiguring port...");
            }
            Ok((rep, _)) => {
                warn!("Device declined baud switch ({}), staying at {}", response_name(rep), old_baud);
                return Ok(false);
            }
            Err(e) => {
                // Timeout here is ambiguous: the device may have switched
                // before ACKing. Fall through, switch the host side and verify.
                warn!("No ACK for baud switch ({}), trying host-side switch + verify...", e);
            }
        }
        self.transport.set_baud_rate(new_baud)?;
        self.baud_rate = new_baud;
        match self.read_partition_table() {
            Ok(_) => {
                info!("Baud switch verified at {}", new_baud);
                Ok(true)
            }
            Err(e) => {
                warn!("Verify at {} failed ({}), restoring {} ...", new_baud, e, old_baud);
                let _ = self.transport.set_baud_rate(old_baud);
                self.baud_rate = old_baud;
                match self.read_partition_table() {
                    Ok(_) => {
                        warn!("Restored old baud, continuing at {}", old_baud);
                        Ok(false)
                    }
                    Err(e2) => Err(anyhow!(
                        "Baud switch failed and session could not be restored ({} / {}); reboot the phone and re-run",
                        e, e2
                    )),
                }
            }
        }
    }

    /// If FDL2 execution triggered a USB re-enumeration / disconnect,
    /// wait for the device to reappear on USB and re-open the serial port.
    pub fn reconnect_if_needed(&mut self, timeout: Duration) -> Result<()> {
        // Probe port health
        let is_broken = self.transport.set_timeout(Duration::from_millis(100)).is_err()
            || self.write_raw(&[]).is_err();

        if is_broken {
            info!("FDL2 triggered USB re-enumeration. Reconnecting...");
            let start = std::time::Instant::now();
            while start.elapsed() < timeout {
                std::thread::sleep(Duration::from_millis(200));
                // Try USB first
                if let Ok(usb) = super::usb_transport::UsbTransport::open_unisoc(Duration::from_millis(200)) {
                    info!("Reconnected successfully to Unisoc device via direct USB!");
                    self.transport = Transport::Usb(usb);
                    std::thread::sleep(Duration::from_millis(300));
                    let _ = self.drain_port();
                    return Ok(());
                }
                // Then try serial (only real Unisoc VID 0x1782)
                if let Some(p) = crate::ports::first_unisoc_serial() {
                    if let Ok(new_port) = serialport::new(&p, self.baud_rate)
                        .timeout(Duration::from_millis(2000))
                        .open()
                    {
                        info!("Reconnected successfully to {}!", p);
                        self.transport = Transport::Serial(new_port);
                        std::thread::sleep(Duration::from_millis(300));
                        let _ = self.drain_port();
                        return Ok(());
                    }
                }
            }
            warn!("Could not detect re-enumerated port within timeout; proceeding with current handle");
        }
        Ok(())
    }

    pub fn reboot(&mut self) -> Result<()> {
        info!("Sending reboot command (BSL_CMD_NORMAL_RESET 0x05)...");
        let (rep, _) = self.send_command(BSL_CMD_NORMAL_RESET, &[])?;
        if rep == BSL_REP_ACK {
            info!("Device is rebooting.");
            Ok(())
        } else {
            Err(anyhow!("Reboot failed: {}", response_name(rep)))
        }
    }

    pub fn power_off(&mut self) -> Result<()> {
        info!("Sending power off command (BSL_CMD_POWER_OFF 0x17)...");
        let (rep, _) = self.send_command(BSL_CMD_POWER_OFF, &[])?;
        if rep == BSL_REP_ACK {
            info!("Device is powering off.");
            Ok(())
        } else {
            Err(anyhow!("Power off failed: {}", response_name(rep)))
        }
    }

    fn drain_port(&mut self) -> Result<()> {
        let mut discard = [0u8; 64];
        let original_timeout = self.transport.timeout();
        self.transport.set_timeout(Duration::from_millis(50))?;
        while let Ok(n) = self.transport.read(&mut discard) {
            if n == 0 {
                break;
            }
        }
        self.transport.set_timeout(original_timeout)?;
        Ok(())
    }
}

fn hex_dump(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<Vec<_>>()
        .join("")
}
