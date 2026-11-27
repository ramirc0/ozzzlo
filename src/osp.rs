//! Ozlo Sleepbuds Protocol (OSP) frames.
//!
//! A frame is `[block, function, operator, length, payload...]`. The same
//! frame travels over USB HID and BLE. Each transport adds its own envelope.

use std::fmt;

/// Largest payload a one-byte length field can describe.
pub const MAX_PAYLOAD: usize = 252;

/// What a frame asks for or reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Operator {
    Set = 0,
    Get = 1,
    SetGet = 2,
    Status = 3,
    Error = 4,
    Start = 5,
    Result = 6,
    Processing = 7,
}

impl Operator {
    fn from_low_bits(byte: u8) -> Option<Self> {
        Some(match byte & 0x0f {
            0 => Self::Set,
            1 => Self::Get,
            2 => Self::SetGet,
            3 => Self::Status,
            4 => Self::Error,
            5 => Self::Start,
            6 => Self::Result,
            7 => Self::Processing,
            _ => return None,
        })
    }
}

/// One addressable function on the case: a function block and an ID within it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Function {
    pub block: u8,
    pub id: u8,
}

/// Functions in the ProductInfo block.
pub mod product_info {
    use super::Function;

    pub const PROTOCOL_VERSION: Function = Function { block: 0, id: 1 };
}

/// Functions in the Status block.
pub mod status {
    use super::Function;

    pub const BATTERY_LEVEL: Function = Function { block: 2, id: 2 };
}

/// A single OSP message, in either direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub function: Function,
    pub operator: Operator,
    pub payload: Vec<u8>,
}

/// Why bytes could not be read as a frame, or a frame could not be written.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameError {
    /// Fewer bytes than the header or the declared payload length.
    Truncated,
    /// The operator nibble is outside the known set.
    UnknownOperator(u8),
    /// The payload exceeds [`MAX_PAYLOAD`].
    PayloadTooLong(usize),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "frame is shorter than its header or length field"),
            Self::UnknownOperator(byte) => write!(f, "unknown operator byte {byte:#04x}"),
            Self::PayloadTooLong(len) => {
                write!(f, "payload of {len} bytes exceeds {MAX_PAYLOAD}")
            }
        }
    }
}

impl std::error::Error for FrameError {}

impl Frame {
    /// A request for the current value of `function`.
    pub fn get(function: Function) -> Self {
        Self {
            function,
            operator: Operator::Get,
            payload: Vec::new(),
        }
    }

    /// Serializes the frame as `[block, function, operator, length, payload...]`.
    ///
    /// Fails if the payload exceeds [`MAX_PAYLOAD`].
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        if self.payload.len() > MAX_PAYLOAD {
            return Err(FrameError::PayloadTooLong(self.payload.len()));
        }
        let mut bytes = vec![
            self.function.block,
            self.function.id,
            self.operator as u8,
            self.payload.len() as u8,
        ];
        bytes.extend_from_slice(&self.payload);
        Ok(bytes)
    }

    /// Parses a frame from `bytes`.
    ///
    /// The high bit of the block byte marks a two-byte big-endian length.
    /// Bytes after the declared payload are ignored.
    pub fn decode(bytes: &[u8]) -> Result<Self, FrameError> {
        let [block, id, operator, rest @ ..] = bytes else {
            return Err(FrameError::Truncated);
        };
        let operator =
            Operator::from_low_bits(*operator).ok_or(FrameError::UnknownOperator(*operator))?;
        let (length, payload) = if block & 0x80 != 0 {
            let [high, low, payload @ ..] = rest else {
                return Err(FrameError::Truncated);
            };
            (usize::from(u16::from_be_bytes([*high, *low])), payload)
        } else {
            let [length, payload @ ..] = rest else {
                return Err(FrameError::Truncated);
            };
            (usize::from(*length), payload)
        };
        let payload = payload.get(..length).ok_or(FrameError::Truncated)?;
        Ok(Self {
            function: Function {
                block: block & 0x7f,
                id: *id,
            },
            operator,
            payload: payload.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_request_matches_the_bytes_the_case_answered() {
        let bytes = Frame::get(status::BATTERY_LEVEL).encode().unwrap();
        assert_eq!(bytes, [0x02, 0x02, 0x01, 0x00]);
    }

    #[test]
    fn decodes_the_captured_battery_reply() {
        let captured = [0x02, 0x02, 0x03, 0x06, 0x64, 0x60, 0x60, 0x00, 0x00, 0x00];
        let frame = Frame::decode(&captured).unwrap();
        assert_eq!(frame.function, status::BATTERY_LEVEL);
        assert_eq!(frame.operator, Operator::Status);
        assert_eq!(frame.payload, [0x64, 0x60, 0x60, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn decodes_the_captured_protocol_version_reply() {
        let captured = [
            0x00, 0x01, 0x03, 0x09, 0x30, 0x30, 0x2e, 0x30, 0x31, 0x2e, 0x30, 0x33, 0x00,
        ];
        let frame = Frame::decode(&captured).unwrap();
        assert_eq!(frame.function, product_info::PROTOCOL_VERSION);
        assert_eq!(frame.payload, b"00.01.03\0");
    }

    #[test]
    fn two_byte_length_is_flagged_by_the_block_high_bit() {
        let mut bytes = vec![0x85, 0x0b, 0x03, 0x01, 0x00];
        bytes.extend(std::iter::repeat_n(0xaa, 256));
        let frame = Frame::decode(&bytes).unwrap();
        assert_eq!(frame.function, Function { block: 5, id: 0x0b });
        assert_eq!(frame.payload.len(), 256);
    }

    #[test]
    fn rejects_a_payload_shorter_than_its_length_field() {
        assert_eq!(
            Frame::decode(&[0x02, 0x02, 0x03, 0x06, 0x64]),
            Err(FrameError::Truncated)
        );
    }

    #[test]
    fn rejects_an_oversized_payload_on_encode() {
        let frame = Frame {
            function: status::BATTERY_LEVEL,
            operator: Operator::Set,
            payload: vec![0; MAX_PAYLOAD + 1],
        };
        assert_eq!(frame.encode(), Err(FrameError::PayloadTooLong(253)));
    }
}
