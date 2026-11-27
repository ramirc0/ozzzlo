//! Adding and verifying the sound files stored on the buds.
//!
//! A sound is one LC3 file per bud. The frames here match a capture of the
//! phone app adding an alarm sound.

use super::{Frame, Function, Malformed, Operator};

pub const BUDS_SOUNDS: Function = Function { block: 5, id: 11 };
pub const ADD: Function = Function { block: 5, id: 29 };
pub const TRANSFER: Function = Function { block: 5, id: 30 };
pub const COMMIT: Function = Function { block: 5, id: 31 };
pub const INTEGRITY_CHECK: Function = Function { block: 5, id: 34 };
pub const UPLOAD_SESSION: Function = Function { block: 5, id: 37 };

/// The L2CAP PSM of the channel that carries an upload.
pub const TRANSFER_PSM: u16 = 129;
/// The file bytes one transfer frame carries. The frame and its envelope fill
/// the 895 bytes the case accepts per packet.
pub const CHUNK_BYTES: usize = 888;

/// Which of a bud's two sound stores a sound lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Kind {
    /// A looping sound for sleep.
    Masking = 0,
    /// A sound an alarm plays.
    Alarm = 1,
}

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

/// A request to make room on `bud` for a new sound `id` of `size` bytes.
pub fn add(id: u8, size: u32, kind: Kind, bud: Bud) -> Frame {
    let mut payload = vec![id];
    payload.extend(size.to_be_bytes());
    payload.extend([kind as u8, bud as u8]);
    Frame {
        function: ADD,
        operator: Operator::Start,
        payload,
    }
}

/// A request to register the transferred file on `bud` as sound `id`.
pub fn commit(id: u8, kind: Kind, bud: Bud) -> Frame {
    Frame::set_get(COMMIT, vec![id, kind as u8, bud as u8])
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
pub fn stored_checksum(bud: Bud, kind: Kind, sound_id: u8) -> Frame {
    Frame {
        function: INTEGRITY_CHECK,
        operator: Operator::Get,
        payload: vec![bud as u8, kind as u8, sound_id],
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
pub fn buds_sounds(bud: Bud, kind: Kind) -> Frame {
    Frame {
        function: BUDS_SOUNDS,
        operator: Operator::Get,
        payload: vec![bud as u8, kind as u8],
    }
}

/// Whether a `SoundInfo` reply shows a slot the bud could store a new sound in.
pub fn has_free_slot(frame: &Frame) -> bool {
    frame.payload.contains(&0xff)
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
    fn requests_match_the_captured_frames() {
        assert_eq!(upload_session_state().to_string(), "05 25 01 00");
        assert_eq!(open_upload_session().to_string(), "05 25 02 01 01");
        assert_eq!(close_upload_session().to_string(), "05 25 02 01 00");
        assert_eq!(
            add(11, 109_818, Kind::Alarm, Bud::Left).to_string(),
            "05 1d 05 07 0b 00 01 ac fa 01 01"
        );
        assert_eq!(transfer(Bud::Left, &[]).to_string(), "05 1e 05 01 01");
        assert_eq!(
            commit(11, Kind::Alarm, Bud::Right).to_string(),
            "05 1f 02 03 0b 01 02"
        );
        assert_eq!(
            stored_checksum(Bud::Left, Kind::Alarm, 11).to_string(),
            "05 22 01 03 01 01 0b"
        );
        assert_eq!(
            buds_sounds(Bud::Right, Kind::Alarm).to_string(),
            "05 0b 01 02 02 01"
        );
    }

    #[test]
    fn a_full_chunk_matches_the_captured_header_and_fills_the_channel() {
        let bytes = transfer(Bud::Left, &[0u8; CHUNK_BYTES])
            .encode_long()
            .unwrap();
        assert_eq!(bytes[..6], [0x85, 0x1e, 0x05, 0x03, 0x79, 0x01]);
        // The case grants 895 bytes per packet, and the envelope takes one.
        assert_eq!(bytes.len(), 894);
    }

    #[test]
    fn a_bud_with_every_slot_taken_has_no_free_slot() {
        let with_room: Frame = "05 0b 03 03 08 09 ff".parse().unwrap();
        let full: Frame = "05 0b 03 03 08 0b 09".parse().unwrap();
        assert!(has_free_slot(&with_room));
        assert!(!has_free_slot(&full));
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
