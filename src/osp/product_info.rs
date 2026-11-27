//! The ProductInfo block: identity and versions.

use super::{Frame, Function, Malformed};

pub const OSP_VERSION: Function = Function { block: 0, id: 1 };
pub const GET_ALL: Function = Function { block: 0, id: 4 };
pub const FIRMWARE_VERSION: Function = Function { block: 0, id: 5 };
pub const SERIAL_NUMBER: Function = Function { block: 0, id: 7 };
pub const HARDWARE_REVISION: Function = Function { block: 0, id: 10 };

/// Firmware versions of the case's two chips and of each bud.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirmwareVersion {
    pub case_bluetooth: String,
    pub case_radio: String,
    pub left: String,
    pub right: String,
}

impl FirmwareVersion {
    /// Reads a `FirmwareVersion` reply: four 13-byte strings.
    pub fn parse(frame: &Frame) -> Result<Self, Malformed> {
        let mut versions = frame.payload.as_chunks::<13>().0.iter().map(|chunk| {
            String::from_utf8_lossy(chunk)
                .trim_end_matches('\0')
                .to_string()
        });
        match (
            versions.next(),
            versions.next(),
            versions.next(),
            versions.next(),
        ) {
            (Some(case_bluetooth), Some(case_radio), Some(left), Some(right)) => Ok(Self {
                case_bluetooth,
                case_radio,
                left,
                right,
            }),
            _ => Err(frame.malformed("four 13-byte version strings")),
        }
    }
}

/// Reads a reply whose whole payload is text, dropping trailing NUL bytes.
pub fn text(frame: &Frame) -> Result<String, Malformed> {
    match std::str::from_utf8(&frame.payload) {
        Ok(text) => Ok(text.trim_end_matches('\0').to_string()),
        Err(_) => Err(frame.malformed("UTF-8 text")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firmware_version_splits_four_fixed_width_strings() {
        let frame: Frame = "00 05 03 34 \
            30 32 2e 31 31 2e 32 36 30 36 30 32 35 \
            30 32 2e 31 31 2e 32 36 30 36 30 30 37 \
            30 32 2e 31 31 2e 32 36 30 36 30 31 31 \
            30 32 2e 31 31 2e 32 36 30 36 30 31 31"
            .parse()
            .unwrap();
        let firmware = FirmwareVersion::parse(&frame).unwrap();
        assert_eq!(firmware.case_bluetooth, "02.11.2606025");
        assert_eq!(firmware.case_radio, "02.11.2606007");
        assert_eq!(firmware.left, "02.11.2606011");
        assert_eq!(firmware.right, "02.11.2606011");
    }

    #[test]
    fn text_drops_the_trailing_nul() {
        let frame: Frame = "00 01 03 09 30 30 2e 30 31 2e 30 33 00".parse().unwrap();
        assert_eq!(text(&frame), Ok("00.01.03".to_string()));
    }
}
