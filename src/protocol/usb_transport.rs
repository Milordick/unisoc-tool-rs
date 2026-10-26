//! Direct USB Bulk Transport for Unisoc/Spreadtrum devices (VID 0x1782).
//!
//! Operates directly via Linux usbfs/nusb, mimicking `spd_dump`.
//! Bypasses serialport and does not require kernel driver binding (option/cdc-acm).

use std::io::{Read, Write};
use std::time::Duration;
use anyhow::{anyhow, Result};
use nusb::transfer::{Bulk, In, Out, ControlOut, ControlType, Direction, Recipient};
use nusb::MaybeFuture;

pub struct UsbTransport {
    _device: nusb::Device,
    _interface: nusb::Interface,
    in_ep: nusb::Endpoint<Bulk, In>,
    out_ep: nusb::Endpoint<Bulk, Out>,
    read_buf: Vec<u8>,
    read_pos: usize,
    timeout: Duration,
}

impl UsbTransport {
    /// Attempts to open a connected Unisoc USB device (VID 0x1782).
    /// If none is currently connected, polls up to `timeout`.
    pub fn open_unisoc(timeout: Duration) -> Result<Self> {
        let start = std::time::Instant::now();
        loop {
            let list = nusb::list_devices().wait()?;
            for dev_info in list {
                if dev_info.vendor_id() == 0x1782 {
                    let dev = dev_info.open().wait()?;
                    let interface = dev.detach_and_claim_interface(0).wait()?;

                    let mut in_addr = None;
                    let mut out_addr = None;

                    for desc in interface.descriptors() {
                        for ep in desc.endpoints() {
                            if ep.transfer_type() == nusb::descriptors::TransferType::Bulk {
                                match ep.direction() {
                                    Direction::In => {
                                        if in_addr.is_none() {
                                            in_addr = Some(ep.address());
                                        }
                                    }
                                    Direction::Out => {
                                        if out_addr.is_none() {
                                            out_addr = Some(ep.address());
                                        }
                                    }
                                }
                            }
                        }
                    }

                    let in_addr = in_addr.ok_or_else(|| anyhow!("Bulk IN endpoint not found on Unisoc device"))?;
                    let out_addr = out_addr.ok_or_else(|| anyhow!("Bulk OUT endpoint not found on Unisoc device"))?;

                    let in_ep = interface.endpoint::<Bulk, In>(in_addr)?;
                    let out_ep = interface.endpoint::<Bulk, Out>(out_addr)?;

                    // Set line state (DTR/RTS) 0x21, 0x22, 0x601 - exact match for spd_dump
                    let _ = interface.control_out(
                        ControlOut {
                            control_type: ControlType::Class,
                            recipient: Recipient::Interface,
                            request: 0x22,
                            value: 0x601,
                            index: 0,
                            data: &[],
                        },
                        Duration::from_millis(500),
                    ).wait();

                    return Ok(Self {
                        _device: dev,
                        _interface: interface,
                        in_ep,
                        out_ep,
                        read_buf: Vec::new(),
                        read_pos: 0,
                        timeout: Duration::from_millis(2000),
                    });
                }
            }

            if start.elapsed() >= timeout {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        Err(anyhow!("No Unisoc USB device (VID 0x1782) found within timeout"))
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }
}

impl Read for UsbTransport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.read_pos < self.read_buf.len() {
            let n = std::cmp::min(buf.len(), self.read_buf.len() - self.read_pos);
            buf[..n].copy_from_slice(&self.read_buf[self.read_pos..self.read_pos + n]);
            self.read_pos += n;
            return Ok(n);
        }

        let alloc_buf = self.in_ep.allocate(4096);
        let completion = self.in_ep.transfer_blocking(alloc_buf, self.timeout);
        match completion.into_result() {
            Ok(data) => {
                if data.is_empty() {
                    return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "Bulk read empty"));
                }
                let n = std::cmp::min(buf.len(), data.len());
                buf[..n].copy_from_slice(&data[..n]);
                if data.len() > n {
                    self.read_buf = data.to_vec();
                    self.read_pos = n;
                } else {
                    self.read_buf.clear();
                    self.read_pos = 0;
                }
                Ok(n)
            }
            Err(e) => Err(std::io::Error::new(std::io::ErrorKind::TimedOut, e.to_string())),
        }
    }
}

impl Write for UsbTransport {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let mut buf = self.out_ep.allocate(data.len());
        buf.extend_from_slice(data);
        let completion = self.out_ep.transfer_blocking(buf, self.timeout);
        match completion.into_result() {
            Ok(_) => Ok(data.len()),
            Err(e) => Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
