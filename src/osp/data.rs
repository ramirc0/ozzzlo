//! The DataCollection block: the sleep and sensor logs the case and buds record.
//!
//! Only the read path is built here. The block's `Clear` function erases a
//! log and is on the hazard list.

use super::{Frame, Function, Malformed, Operator};

pub const READ_INIT: Function = Function { block: 13, id: 2 };
pub const READ_TRANSFER: Function = Function { block: 13, id: 3 };

/// Whose log to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorder {
    Case = 0,
    LeftBud = 1,
    RightBud = 2,
}

/// Where a recorder's log sits in its storage, from a `ReadInit` reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogExtent {
    pub start_offset: u32,
    pub size: u32,
}

impl LogExtent {
    /// Reads a `ReadInit` reply: recorder, start offset, data size, storage size.
    ///
    /// # Errors
    ///
    /// Returns [`Malformed`] if the payload does not fit that layout.
    pub fn parse(frame: &Frame) -> Result<Self, Malformed> {
        match frame.payload[..] {
            [_recorder, s0, s1, s2, s3, d0, d1, d2, d3, ..] => Ok(Self {
                start_offset: u32::from_be_bytes([s0, s1, s2, s3]),
                size: u32::from_be_bytes([d0, d1, d2, d3]),
            }),
            _ => Err(frame.malformed("recorder, 32-bit start offset, 32-bit size")),
        }
    }
}

/// A request to open `recorder`'s log for reading.
pub fn open_log(recorder: Recorder) -> Frame {
    Frame {
        function: READ_INIT,
        operator: Operator::Start,
        payload: vec![recorder as u8],
    }
}

/// A request for `count` bytes of `recorder`'s log, starting at `offset`.
pub fn log_bytes(recorder: Recorder, offset: u32, count: u32) -> Frame {
    let mut payload = vec![recorder as u8];
    payload.extend(offset.to_be_bytes());
    payload.extend(count.to_be_bytes());
    Frame {
        function: READ_TRANSFER,
        operator: Operator::Start,
        payload,
    }
}

/// Reads a `ReadTransfer` reply and returns the log bytes it carries.
///
/// The payload is recorder, offset, size read, then that many bytes.
///
/// # Errors
///
/// Returns [`Malformed`] if the payload does not fit that layout.
pub fn chunk(frame: &Frame) -> Result<&[u8], Malformed> {
    let expected = "recorder, 32-bit offset, 32-bit size, then that many bytes";
    let [_recorder, _, _, _, _, n0, n1, n2, n3, data @ ..] = &frame.payload[..] else {
        return Err(frame.malformed(expected));
    };
    let size = u32::from_be_bytes([*n0, *n1, *n2, *n3]) as usize;
    if size == 0 || size != data.len() {
        return Err(frame.malformed(expected));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_match_the_vendor_frames() {
        assert_eq!(open_log(Recorder::Case).to_string(), "0d 02 05 01 00");
        assert_eq!(
            log_bytes(Recorder::RightBud, 0x0102, 200).to_string(),
            "0d 03 05 09 02 00 00 01 02 00 00 00 c8"
        );
    }

    #[test]
    fn extent_reads_offset_then_size() {
        let frame: Frame = "0d 02 06 0d 00 00 00 00 10 00 00 3c cf 00 10 00 00"
            .parse()
            .unwrap();
        let expected = LogExtent {
            start_offset: 16,
            size: 15567,
        };
        assert_eq!(LogExtent::parse(&frame), Ok(expected));
    }

    #[test]
    fn chunk_returns_the_bytes_after_the_header() {
        let frame: Frame = "0d 03 06 0c 00 00 00 00 10 00 00 00 03 aa bb cc"
            .parse()
            .unwrap();
        assert_eq!(chunk(&frame), Ok(&[0xaa, 0xbb, 0xcc][..]));
    }

    #[test]
    fn chunk_rejects_a_size_that_disagrees_with_the_data() {
        let frame: Frame = "0d 03 06 0b 00 00 00 00 10 00 00 00 03 aa bb"
            .parse()
            .unwrap();
        assert!(chunk(&frame).is_err());
    }
}
