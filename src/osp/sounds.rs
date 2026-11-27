//! Replacing and verifying the sound files stored on the buds.
//!
//! A sound is one LC3 file per bud. The frames here follow the vendor's
//! Android app. Only replacing an existing sound is built.

use super::{Frame, Function, Malformed, Operator};

pub const BUDS_SOUNDS: Function = Function { block: 5, id: 11 };
pub const TRANSFER: Function = Function { block: 5, id: 30 };
pub const INTEGRITY_CHECK: Function = Function { block: 5, id: 34 };
pub const REPLACE: Function = Function { block: 5, id: 36 };
pub const UPLOAD_SESSION: Function = Function { block: 5, id: 37 };

/// The audio bytes one transfer frame carries. A frame and its envelope must
/// fit one write at the usual MTU.
pub const CHUNK_BYTES: usize = 235;
/// The vendor's masking sound type.
pub const MASKING: u8 = 0;

/// One of the two buds, as the case numbers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bud {
    Left = 1,
    Right = 2,
}

/// The checksum a bud computes over a stored sound file.
///
/// It is a CRC-32 with polynomial `04C11DB7`, a zero start value, no bit
/// reflection and no final inversion.
pub fn checksum(file: &[u8]) -> u32 {
    let mut crc: u32 = 0;
    for byte in file {
        crc ^= u32::from(*byte) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// Checks that `file` is an LC3 file with the settings the vendor's sounds use:
/// 48 kHz, 96 kbit/s, mono, 10 ms frames, and whole frames to the last byte.
///
/// # Errors
///
/// Returns what is wrong with the file, in words for the user.
pub fn vet(file: &[u8]) -> Result<(), String> {
    let field = |index: usize| -> Option<u16> {
        let bytes = file.get(index * 2..index * 2 + 2)?;
        Some(u16::from_le_bytes([bytes[0], bytes[1]]))
    };
    let header = (field(0), field(1), field(2), field(3), field(4), field(5));
    let (Some(magic), Some(header_bytes), Some(rate), Some(bitrate), Some(channels), Some(frame)) =
        header
    else {
        return Err("the file is too short to be an LC3 file".to_string());
    };
    if magic != 0xcc1c {
        return Err("the file does not start with the LC3 file marker".to_string());
    }
    if (rate, bitrate, channels, frame) != (480, 960, 1, 1000) {
        return Err(format!(
            "the file is {} Hz, {} bit/s, {channels} channel(s), {} ms frames. \
             The buds' sounds are 48000 Hz, 96000 bit/s, 1 channel, 10 ms",
            u32::from(rate) * 100,
            u32::from(bitrate) * 100,
            f32::from(frame) / 100.0
        ));
    }
    let mut offset = usize::from(header_bytes);
    while offset < file.len() {
        let Some(size) = file.get(offset..offset + 2) else {
            return Err("the file ends inside a frame header".to_string());
        };
        offset += 2 + usize::from(u16::from_le_bytes([size[0], size[1]]));
    }
    if offset == file.len() {
        Ok(())
    } else {
        Err("the file ends inside a frame".to_string())
    }
}

/// A request for whether an upload session is open.
pub fn upload_session_state() -> Frame {
    Frame::get(UPLOAD_SESSION)
}

/// A request to open the upload session that sound transfers run inside.
pub fn open_upload_session() -> Frame {
    Frame::set_get(UPLOAD_SESSION, vec![1])
}

/// A request to close the upload session, which also cancels a transfer.
pub fn close_upload_session() -> Frame {
    Frame::set_get(UPLOAD_SESSION, vec![0])
}

/// A request to overwrite sound `old_id` on `bud` with a file of `size` bytes
/// that will be known as `new_id`.
pub fn replace(old_id: u8, new_id: u8, size: u32, kind: u8, bud: Bud) -> Frame {
    let mut payload = vec![old_id, new_id];
    payload.extend(size.to_be_bytes());
    payload.extend([kind, bud as u8]);
    Frame {
        function: REPLACE,
        operator: Operator::Start,
        payload,
    }
}

/// A transfer frame that carries the next bytes of the file to `bud`.
///
/// An empty `bytes` marks the end of the file.
pub fn transfer(bud: Bud, bytes: &[u8]) -> Frame {
    let mut payload = vec![bud as u8];
    payload.extend_from_slice(bytes);
    Frame {
        function: TRANSFER,
        operator: Operator::Start,
        payload,
    }
}

/// A request for the checksum `bud` computes over its stored copy of a sound.
pub fn stored_checksum(bud: Bud, kind: u8, sound_id: u8) -> Frame {
    Frame {
        function: INTEGRITY_CHECK,
        operator: Operator::Get,
        payload: vec![bud as u8, kind, sound_id],
    }
}

/// Reads an `IntegrityCheck` reply: a 32-bit checksum.
///
/// # Errors
///
/// Returns [`Malformed`] if the payload does not fit that layout.
pub fn parse_checksum(frame: &Frame) -> Result<u32, Malformed> {
    match frame.payload[..] {
        [a, b, c, d] => Ok(u32::from_be_bytes([a, b, c, d])),
        _ => Err(frame.malformed("a 32-bit checksum")),
    }
}

/// Reads a `SoundInfo` reply: the sound IDs a bud holds. `0xFF` slots are empty.
pub fn stored_ids(frame: &Frame) -> Vec<u8> {
    frame
        .payload
        .iter()
        .copied()
        .filter(|id| *id != 0xff)
        .collect()
}

/// A request for the sound IDs of `kind` that `bud` holds.
pub fn buds_sounds(bud: Bud, kind: u8) -> Frame {
    Frame {
        function: BUDS_SOUNDS,
        operator: Operator::Get,
        payload: vec![bud as u8, kind],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The header of a vendor sound file, then two empty-bodied frames.
    const HEADER: [u8; 18] = [
        0x1c, 0xcc, 0x12, 0x00, 0xe0, 0x01, 0xc0, 0x03, 0x01, 0x00, 0xe8, 0x03, 0x00, 0x00, 0x00,
        0x2f, 0x0d, 0x00,
    ];

    fn file_with_frames(frames: usize) -> Vec<u8> {
        let mut file = HEADER.to_vec();
        for _ in 0..frames {
            file.extend([0x78, 0x00]);
            file.extend([0u8; 120]);
        }
        file
    }

    #[test]
    fn checksum_matches_the_reference_value_for_this_variant() {
        assert_eq!(checksum(b"123456789"), 0x89A1_897F);
        assert_eq!(checksum(b""), 0);
    }

    #[test]
    fn a_vendor_shaped_file_passes_vetting() {
        assert_eq!(vet(&file_with_frames(3)), Ok(()));
    }

    #[test]
    fn vetting_rejects_wrong_marker_settings_and_truncation() {
        let mut wrong_marker = file_with_frames(1);
        wrong_marker[0] = 0x52;
        assert!(vet(&wrong_marker).unwrap_err().contains("marker"));

        let mut wrong_rate = file_with_frames(1);
        wrong_rate[4..6].copy_from_slice(&441u16.to_le_bytes());
        assert!(vet(&wrong_rate).unwrap_err().contains("44100 Hz"));

        let mut truncated = file_with_frames(2);
        truncated.truncate(truncated.len() - 5);
        assert!(vet(&truncated).unwrap_err().contains("inside a frame"));

        assert!(vet(&HEADER[..7]).is_err());
    }

    #[test]
    fn requests_match_the_vendor_frames() {
        assert_eq!(upload_session_state().to_string(), "05 25 01 00");
        assert_eq!(open_upload_session().to_string(), "05 25 02 01 01");
        assert_eq!(close_upload_session().to_string(), "05 25 02 01 00");
        assert_eq!(
            replace(65, 65, 353_696, MASKING, Bud::Left).to_string(),
            "05 24 05 08 41 41 00 05 65 a0 00 01"
        );
        assert_eq!(
            transfer(Bud::Right, &[0xaa, 0xbb]).to_string(),
            "05 1e 05 03 02 aa bb"
        );
        assert_eq!(transfer(Bud::Left, &[]).to_string(), "05 1e 05 01 01");
        assert_eq!(
            stored_checksum(Bud::Left, MASKING, 18).to_string(),
            "05 22 01 03 01 00 12"
        );
        assert_eq!(
            buds_sounds(Bud::Right, MASKING).to_string(),
            "05 0b 01 02 02 00"
        );
    }

    #[test]
    fn a_full_chunk_fits_one_frame() {
        let frame = transfer(Bud::Left, &[0u8; CHUNK_BYTES]);
        assert!(frame.encode().is_ok());
    }

    #[test]
    fn replies_parse_as_captured() {
        let ids: Frame = "05 0b 03 0a 00 01 03 05 06 07 0a 12 25 41".parse().unwrap();
        assert_eq!(stored_ids(&ids), [0, 1, 3, 5, 6, 7, 10, 18, 37, 65]);
        let with_gap: Frame = "05 0b 03 03 08 ff 09".parse().unwrap();
        assert_eq!(stored_ids(&with_gap), [8, 9]);
        let checksum: Frame = "05 22 03 04 dc fb a4 51".parse().unwrap();
        assert_eq!(parse_checksum(&checksum), Ok(0xDCFB_A451));
    }
}
