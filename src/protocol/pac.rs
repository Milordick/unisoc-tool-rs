#![allow(dead_code)]

use anyhow::{anyhow, Context, Result};
use byteorder::{ByteOrder, LittleEndian};
use log::{info, warn};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

#[allow(dead_code)]

pub const PAC_HEADER_SIZE: usize = 2124;
pub const PAC_FILE_HEADER_SIZE: usize = 2580;
pub const PAC_MAGIC: u32 = 0xFFFA_FFFA;

// Pac Header Offsets
const OFF_PAC_SIZE: usize = 48;
const OFF_PRD_NAME: usize = 52;        // 256 uint16
const OFF_PRD_VERSION: usize = 564;     // 256 uint16
const OFF_FILE_COUNT: usize = 1076;    // uint32
const OFF_FILE_OFFSET: usize = 1080;   // uint32
const OFF_MODE: usize = 1084;          // uint32
const OFF_FLASH_TYPE: usize = 1088;    // uint32
const OFF_PRD_ALIAS: usize = 1108;     // 100 uint16
const OFF_MAGIC: usize = 2116;         // uint32
const OFF_CRC1: usize = 2120;          // uint16
const OFF_CRC2: usize = 2122;          // uint16

// Pac File Header Offsets
const FOFF_FILE_ID: usize = 4;         // 256 uint16
const FOFF_FILE_NAME: usize = 516;     // 256 uint16
const FOFF_DATA_SIZE: usize = 1540;    // uint32
const FOFF_FLAG: usize = 1544;         // uint32
const FOFF_DATA_OFFSET: usize = 1552;  // uint32
const FOFF_OMIT: usize = 1556;         // uint32
const FOFF_ADDRESS: usize = 1564;      // uint32

#[derive(Debug, Clone)]
pub struct PacFileEntry {
    pub file_id: String,
    pub file_name: String,
    pub size: u64,
    pub offset: u64,
    pub address: u32,
    pub flag: u32,
    pub omit: u32,
}

impl PacFileEntry {
    pub fn is_marker(&self) -> bool {
        self.size == 0
    }

    pub fn is_fdl1(&self) -> bool {
        let lower_id = self.file_id.to_lowercase();
        let lower_name = self.file_name.to_lowercase();
        lower_id.contains("fdl1")
            || (lower_id == "fdl" && !lower_name.contains("fdl2"))
            || lower_id == "host_fdl"
            || lower_name.contains("fdl1")
    }

    pub fn is_fdl2(&self) -> bool {
        let lower_id = self.file_id.to_lowercase();
        let lower_name = self.file_name.to_lowercase();
        lower_id.contains("fdl2") || lower_name.contains("fdl2")
    }
}

#[derive(Debug, Clone)]
pub struct PacPackage {
    pub version: String,
    pub product_name: String,
    pub product_version: String,
    pub product_alias: String,
    pub total_size: u64,
    pub mode: u32,
    pub flash_type: u32,
    pub magic: u32,
    pub entries: Vec<PacFileEntry>,
}

fn decode_utf16_le(raw: &[u8]) -> String {
    let mut s = String::new();
    let mut i = 0;
    while i + 1 < raw.len() {
        let val = LittleEndian::read_u16(&raw[i..i + 2]);
        if val == 0 {
            break;
        }
        if let Some(c) = char::from_u32(val as u32) {
            s.push(c);
        } else {
            s.push('?');
        }
        i += 2;
    }
    s.trim().to_string()
}

impl PacPackage {
    pub fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)
            .with_context(|| format!("Failed to open PAC file: {}", path.display()))?;
        
        let metadata = file.metadata()?;
        let file_len = metadata.len();
        if file_len < PAC_HEADER_SIZE as u64 {
            return Err(anyhow!("File is too small to be a valid PAC package"));
        }

        let mut header = [0u8; PAC_HEADER_SIZE];
        file.read_exact(&mut header)?;

        let version = decode_utf16_le(&header[..48]);
        // Kept for documentation only: the header's pac_size is a 32-bit
        // value that wraps past 4 GiB (see total_size comment below).
        let _pac_size = LittleEndian::read_u32(&header[OFF_PAC_SIZE..OFF_PAC_SIZE + 4]) as u64;
        let product_name = decode_utf16_le(&header[OFF_PRD_NAME..OFF_PRD_NAME + 512]);
        let product_version = decode_utf16_le(&header[OFF_PRD_VERSION..OFF_PRD_VERSION + 512]);
        let file_count = LittleEndian::read_u32(&header[OFF_FILE_COUNT..OFF_FILE_COUNT + 4]) as usize;
        let file_offset = LittleEndian::read_u32(&header[OFF_FILE_OFFSET..OFF_FILE_OFFSET + 4]) as u64;
        let mode = LittleEndian::read_u32(&header[OFF_MODE..OFF_MODE + 4]);
        let flash_type = LittleEndian::read_u32(&header[OFF_FLASH_TYPE..OFF_FLASH_TYPE + 4]);
        let product_alias = decode_utf16_le(&header[OFF_PRD_ALIAS..OFF_PRD_ALIAS + 200]);
        let magic = LittleEndian::read_u32(&header[OFF_MAGIC..OFF_MAGIC + 4]);

        if file_count > 1000 {
            return Err(anyhow!("Unreasonable file count in PAC header: {}", file_count));
        }

        // Magic verified against a real stock .pac (V152: 0xFFFAFFFA).
        // A mismatch does not abort parsing — other SPD generations may
        // differ — but it is a strong hint the file is not a .pac at all.
        if magic != PAC_MAGIC {
            warn!(
                "Unexpected PAC magic 0x{:08X} (expected 0x{:08X}); parsing anyway, but the file may not be a valid .pac",
                magic, PAC_MAGIC
            );
        }

        // Seek to file table start
        file.seek(SeekFrom::Start(file_offset))?;
        let mut entries = Vec::with_capacity(file_count);

        for idx in 0..file_count {
            let mut f_header = [0u8; PAC_FILE_HEADER_SIZE];
            file.read_exact(&mut f_header)
                .with_context(|| format!("Failed to read file header #{}", idx))?;

            let file_id = decode_utf16_le(&f_header[FOFF_FILE_ID..FOFF_FILE_ID + 512]);
            let file_name = decode_utf16_le(&f_header[FOFF_FILE_NAME..FOFF_FILE_NAME + 512]);
            let size = LittleEndian::read_u32(&f_header[FOFF_DATA_SIZE..FOFF_DATA_SIZE + 4]) as u64;
            let flag = LittleEndian::read_u32(&f_header[FOFF_FLAG..FOFF_FLAG + 4]);
            let data_offset = LittleEndian::read_u32(&f_header[FOFF_DATA_OFFSET..FOFF_DATA_OFFSET + 4]) as u64;
            let omit = LittleEndian::read_u32(&f_header[FOFF_OMIT..FOFF_OMIT + 4]);
            let address = LittleEndian::read_u32(&f_header[FOFF_ADDRESS..FOFF_ADDRESS + 4]);

            entries.push(PacFileEntry {
                file_id,
                file_name,
                size,
                offset: data_offset,
                address,
                flag,
                omit,
            });
        }

        Ok(Self {
            version,
            product_name,
            product_version,
            product_alias,
            // The header's pac_size field is a 32-bit value: it wraps around
            // for packages over 4 GiB (V152's real 4,552,938,116 bytes show up
            // as 257,970,820). The real file size is the truthful total.
            total_size: file_len,
            mode,
            flash_type,
            magic,
            entries,
        })
    }

    pub fn find_fdl1(&self) -> Option<&PacFileEntry> {
        self.entries.iter().find(|e| e.is_fdl1())
    }

    pub fn find_fdl2(&self) -> Option<&PacFileEntry> {
        self.entries.iter().find(|e| e.is_fdl2())
    }

    pub fn read_entry_bytes(&self, file: &mut File, entry: &PacFileEntry) -> Result<Vec<u8>> {
        if entry.size == 0 {
            return Ok(Vec::new());
        }
        file.seek(SeekFrom::Start(entry.offset))?;
        let mut buf = vec![0u8; entry.size as usize];
        file.read_exact(&mut buf)?;
        Ok(buf)
    }

    pub fn unpack_all(&self, pac_path: &Path, out_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(out_dir)?;
        let mut file = File::open(pac_path)?;

        info!("Extracting {} entries to {:?}...", self.entries.len(), out_dir);
        for (i, entry) in self.entries.iter().enumerate() {
            if entry.size == 0 || entry.flag == 0 {
                continue;
            }

            let file_name = if !entry.file_name.is_empty() {
                &entry.file_name
            } else if !entry.file_id.is_empty() {
                &entry.file_id
            } else {
                continue;
            };

            // Sanitize filename
            let safe_name = file_name.replace(['/', '\\', ':', '*', '?', '"', '<', '>', '|'], "_");
            let target_path = out_dir.join(&safe_name);

            file.seek(SeekFrom::Start(entry.offset))?;
            let mut out = File::create(&target_path)?;

            let mut remaining = entry.size;
            let mut buf = vec![0u8; 65536];
            while remaining > 0 {
                let to_read = std::cmp::min(remaining, buf.len() as u64) as usize;
                file.read_exact(&mut buf[..to_read])?;
                out.write_all(&buf[..to_read])?;
                remaining -= to_read as u64;
            }

            info!(
                "[{}/{}] Extracted {:<20} -> {} ({} bytes)",
                i + 1,
                self.entries.len(),
                entry.file_id,
                safe_name,
                entry.size
            );
        }

        Ok(())
    }
}
