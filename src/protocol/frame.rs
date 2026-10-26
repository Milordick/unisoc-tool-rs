#![allow(dead_code)]

use anyhow::{anyhow, Result};
use byteorder::{BigEndian, ByteOrder};
use log::warn;
use super::constants::*;

/// Spreadtrum ones-complement word checksum (official Unisoc algorithm)
pub fn calculate_spd_checksum(data: &[u8]) -> u16 {
    let mut crc: u32 = 0;
    let mut i = 0;
    while data.len().saturating_sub(i) > 1 {
        crc += ((data[i + 1] as u32) << 8) | (data[i] as u32);
        i += 2;
    }
    if i < data.len() {
        crc += data[i] as u32;
    }
    crc = (crc >> 16) + (crc & 0xFFFF);
    crc += crc >> 16;
    crc = (!crc) & 0xFFFF;
    ((crc >> 8) | ((crc & 0xFF) << 8)) as u16
}

/// Classic BootROM CRC-16-CCITT (poly 0x1021, init 0, MSB-first)
pub fn calculate_crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            if (crc & 0x8000) != 0 {
                crc = (crc << 1) ^ 0x1021;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

pub fn encode_frame(cmd_type: u16, payload: &[u8], use_crc: bool) -> Vec<u8> {
    let mut raw = Vec::with_capacity(4 + payload.len() + 2);
    let mut header = [0u8; 4];
    BigEndian::write_u16(&mut header[0..2], cmd_type);
    BigEndian::write_u16(&mut header[2..4], payload.len() as u16);
    raw.extend_from_slice(&header);
    raw.extend_from_slice(payload);

    let check = if use_crc {
        calculate_crc16(&raw)
    } else {
        calculate_spd_checksum(&raw)
    };

    let mut check_bytes = [0u8; 2];
    BigEndian::write_u16(&mut check_bytes, check);
    raw.extend_from_slice(&check_bytes);

    // Escape and wrap with HDLC_FLAG
    let mut framed = Vec::with_capacity(raw.len() * 2 + 2);
    framed.push(HDLC_FLAG);
    for b in raw {
        if b == HDLC_FLAG || b == HDLC_ESCAPE {
            framed.push(HDLC_ESCAPE);
            framed.push(b ^ HDLC_ESCAPE_MASK);
        } else {
            framed.push(b);
        }
    }
    framed.push(HDLC_FLAG);
    framed
}

pub fn decode_frame(framed: &[u8], use_crc: bool, strict_checksum: bool) -> Result<(u16, Vec<u8>)> {
    if framed.len() < 8 {
        return Err(anyhow!("Frame too short: {} bytes", framed.len()));
    }
    if framed[0] != HDLC_FLAG || framed[framed.len() - 1] != HDLC_FLAG {
        return Err(anyhow!("Missing HDLC start or end flag"));
    }

    // Unescape inner bytes
    let inner = &framed[1..framed.len() - 1];
    let mut unescaped = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        if inner[i] == HDLC_ESCAPE {
            i += 1;
            if i >= inner.len() {
                return Err(anyhow!("Trailing escape byte"));
            }
            unescaped.push(inner[i] ^ HDLC_ESCAPE_MASK);
        } else {
            unescaped.push(inner[i]);
        }
        i += 1;
    }

    if unescaped.len() < 6 {
        return Err(anyhow!("Unescaped data too short: {} bytes", unescaped.len()));
    }

    let cmd_type = BigEndian::read_u16(&unescaped[0..2]);
    let len = BigEndian::read_u16(&unescaped[2..4]) as usize;

    if unescaped.len() < 4 + len + 2 {
        return Err(anyhow!(
            "Payload length mismatch: declared {}, total unescaped {}",
            len,
            unescaped.len()
        ));
    }

    let payload = unescaped[4..4 + len].to_vec();

    // Verify the trailing checksum (same algorithm as the sender negotiated).
    // Default policy is warn-and-accept: some FDL2 builds answer with the
    // other checksum variant, and rejecting their frames would break the
    // session entirely. With strict_checksum (UNISOC_STRICT_CHECKSUM=1) a
    // mismatch is fatal instead — useful when diagnosing a flaky link.
    let transmitted = BigEndian::read_u16(&unescaped[4 + len..4 + len + 2]);
    let expected = if use_crc {
        calculate_crc16(&unescaped[0..4 + len])
    } else {
        calculate_spd_checksum(&unescaped[0..4 + len])
    };
    if transmitted != expected {
        if strict_checksum {
            return Err(anyhow!(
                "Frame checksum mismatch for cmd 0x{:04X}: got 0x{:04X}, expected 0x{:04X} ({} mode)",
                cmd_type,
                transmitted,
                expected,
                if use_crc { "CRC16" } else { "SPD" }
            ));
        }
        warn!(
            "Frame checksum mismatch for cmd 0x{:04X}: got 0x{:04X}, expected 0x{:04X} ({} mode) — accepting anyway",
            cmd_type,
            transmitted,
            expected,
            if use_crc { "CRC16" } else { "SPD" }
        );
    }

    Ok((cmd_type, payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spd_checksum() {
        let vec = [0x00, 0x00, 0x00, 0x00];
        assert_eq!(calculate_spd_checksum(&vec), 0xFFFF);
    }

    #[test]
    fn test_roundtrip_basic() {
        let cmd = BSL_CMD_CONNECT;
        let payload = b"TEST_PAYLOAD";
        let encoded = encode_frame(cmd, payload, false);
        assert_eq!(encoded[0], HDLC_FLAG);
        assert_eq!(encoded[encoded.len() - 1], HDLC_FLAG);

        let (decoded_cmd, decoded_payload) = decode_frame(&encoded, false, false).unwrap();
        assert_eq!(decoded_cmd, cmd);
        assert_eq!(decoded_payload, payload);
    }

    #[test]
    fn test_escape_characters() {
        let cmd = 0x1234;
        let payload = vec![HDLC_FLAG, HDLC_ESCAPE, 0x00, HDLC_FLAG, 0xAA];
        let encoded = encode_frame(cmd, &payload, false);
        // Ensure no raw 0x7E or 0x7D exist in the middle
        for &b in &encoded[1..encoded.len() - 1] {
            assert_ne!(b, HDLC_FLAG);
        }

        let (decoded_cmd, decoded_payload) = decode_frame(&encoded, false, false).unwrap();
        assert_eq!(decoded_cmd, cmd);
        assert_eq!(decoded_payload, payload);
    }

    #[test]
    fn test_crc16_roundtrip() {
        let cmd = BSL_CMD_START_DATA;
        let payload = vec![0x12, 0x34, 0x56, 0x78];
        let encoded = encode_frame(cmd, &payload, true);
        let (decoded_cmd, decoded_payload) = decode_frame(&encoded, true, false).unwrap();
        assert_eq!(decoded_cmd, cmd);
        assert_eq!(decoded_payload, payload);
    }

    #[test]
    fn test_checksum_mismatch_is_warned_not_fatal() {
        // Corrupt one PAYLOAD byte after encoding: the frame must still decode
        // (warn-and-accept policy for FDL2 builds using the other checksum).
        // Framed layout here: [7E] 00 00 00 05 'H' 'E' 'L' 'L' 'O' chk chk [7E]
        // (header/payload contain no flag/escape bytes, so no escaping shifts
        // the offsets) — index 5 is the first payload byte.
        let cmd = BSL_CMD_CONNECT;
        let payload = b"HELLO";
        let mut encoded = encode_frame(cmd, payload, false);
        assert_eq!(encoded[5], b'H');
        encoded[5] ^= 0xFF;
        let (decoded_cmd, _) = decode_frame(&encoded, false, false).unwrap();
        assert_eq!(decoded_cmd, cmd);
    }

    #[test]
    fn test_checksum_mismatch_is_fatal_in_strict_mode() {
        let cmd = BSL_CMD_CONNECT;
        let payload = b"HELLO";
        let mut encoded = encode_frame(cmd, payload, false);
        assert_eq!(encoded[5], b'H');
        encoded[5] ^= 0xFF;
        assert!(decode_frame(&encoded, false, true).is_err());
    }
}
