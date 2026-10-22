//! Firmware sends one protocol reply in consecutive ATT notifications (MTU - 3).
//! Binary readback payloads may contain the same bytes as the frame delimiters.
use crate::{
    ahakey::x1::commands::valid_frame,
    error::{Error, Result},
};
use std::collections::VecDeque;

// command_return_frame() takes a uint8_t length. Bound even incomplete input.
const MAX_FRAME_BYTES: usize = u8::MAX as usize;

#[derive(Default)]
pub(super) struct Frames {
    partial: Vec<u8>,
    ready: VecDeque<Vec<u8>>,
}

enum Boundary {
    Pending,
    Length(usize),
    Delimiter,
}

impl Frames {
    pub fn next(&mut self) -> Option<Vec<u8>> {
        self.ready.pop_front()
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<()> {
        for &byte in bytes {
            match self.partial.as_slice() {
                [] => {
                    if byte == 0xaa {
                        self.partial.push(byte);
                    }
                    continue;
                }
                [0xaa] => {
                    if byte == 0xbb {
                        self.partial.push(byte);
                    } else if byte != 0xaa {
                        self.partial.clear();
                    }
                    continue;
                }
                _ => self.partial.push(byte),
            }
            if self.partial.len() > MAX_FRAME_BYTES {
                return Err(Error("DEVICE_INVALID_RESPONSE"));
            }
            let complete = match boundary(&self.partial) {
                Boundary::Pending => false,
                Boundary::Length(length) => {
                    if length > MAX_FRAME_BYTES {
                        return Err(Error("DEVICE_INVALID_RESPONSE"));
                    }
                    if self.partial.len() < length {
                        false
                    } else if !valid_frame(&self.partial) {
                        return Err(Error("DEVICE_INVALID_RESPONSE"));
                    } else {
                        true
                    }
                }
                Boundary::Delimiter => valid_frame(&self.partial),
            };
            if complete {
                self.ready.push_back(std::mem::take(&mut self.partial));
            }
        }
        Ok(())
    }
}

fn boundary(bytes: &[u8]) -> Boundary {
    if bytes.len() < 4 {
        return Boundary::Pending;
    }
    // Error replies have no typed binary payload. Leave status validation to
    // response()/observe(), including rejection of malformed long errors.
    if bytes[2] != 0 && bytes[3] != 0 {
        return Boundary::Delimiter;
    }
    // Generic success ACKs precede typed replies on older firmware.
    if bytes.len() == 6 && bytes[4..] == [0xcc, 0xdd] {
        return Boundary::Length(6);
    }
    match bytes[2] {
        0x87 => {
            let Some(&action_len) = bytes.get(7) else {
                return Boundary::Pending;
            };
            let description_offset = 8 + action_len as usize;
            if description_offset + 3 > MAX_FRAME_BYTES {
                return Boundary::Length(description_offset + 3);
            }
            match bytes.get(description_offset) {
                Some(&description_len) => {
                    Boundary::Length(description_offset + 3 + description_len as usize)
                }
                None => Boundary::Pending,
            }
        }
        0x88 => match bytes.get(4) {
            None => Boundary::Pending,
            Some(0) => Boundary::Length(8),
            Some(1 | 2) => Boundary::Length(10),
            Some(3) => Boundary::Length(18),
            _ => Boundary::Delimiter,
        },
        0x97 => {
            let Some(&positions) = bytes.get(5) else {
                return Boundary::Pending;
            };
            let mut offset = 6;
            for _ in 0..positions {
                let Some(&action_len) = bytes.get(offset + 1) else {
                    return Boundary::Pending;
                };
                offset += 2 + action_len as usize;
                if offset + 2 > MAX_FRAME_BYTES {
                    return Boundary::Length(offset + 2);
                }
            }
            Boundary::Length(offset + 2)
        }
        // Catalog text and current status/ACK payloads have no embedded CC DD.
        _ => Boundary::Delimiter,
    }
}
