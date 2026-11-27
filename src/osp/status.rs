//! The Status block: live state of the case and buds.

use super::{Frame, Function, Malformed};

pub const GET_ALL: Function = Function { block: 2, id: 1 };
pub const BATTERY_LEVEL: Function = Function { block: 2, id: 2 };
pub const CHARGING_STATE: Function = Function { block: 2, id: 5 };
pub const BUDS_LOCATION: Function = Function { block: 2, id: 8 };
pub const BUDS_CONNECTION_STATE: Function = Function { block: 2, id: 9 };
pub const CASE_LID_STATE: Function = Function { block: 2, id: 15 };

/// Charge of the case and each bud, in percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryLevel {
    pub case: u8,
    pub left: u8,
    pub right: u8,
}

impl BatteryLevel {
    /// Reads a `BatteryLevel` reply.
    pub fn parse(frame: &Frame) -> Result<Self, Malformed> {
        match frame.payload[..] {
            [case, left, right, ..] => Ok(Self { case, left, right }),
            _ => Err(frame.malformed("case, left and right levels")),
        }
    }
}

/// Where the buds are relative to the case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudsLocation {
    InCase,
    OutOfCase,
    LeftOut,
    RightOut,
}

impl BudsLocation {
    /// Reads a `BudsLocation` reply.
    pub fn parse(frame: &Frame) -> Result<Self, Malformed> {
        match frame.payload[..] {
            [0] => Ok(Self::InCase),
            [1] => Ok(Self::OutOfCase),
            [2] => Ok(Self::LeftOut),
            [3] => Ok(Self::RightOut),
            _ => Err(frame.malformed("one location byte from 0 to 3")),
        }
    }
}

/// Signal strength of each bud's link to the case, in dBm.
///
/// A bud without a link is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudsConnection {
    pub left: Option<i16>,
    pub right: Option<i16>,
}

impl BudsConnection {
    /// Reads a `BudsConnectionState` reply.
    pub fn parse(frame: &Frame) -> Result<Self, Malformed> {
        let [left, l_high, l_low, right, r_high, r_low] = frame.payload[..] else {
            return Err(frame.malformed("a connected flag and a 16-bit RSSI per bud"));
        };
        Ok(Self {
            left: (left == 1).then(|| i16::from_be_bytes([l_high, l_low])),
            right: (right == 1).then(|| i16::from_be_bytes([r_high, r_low])),
        })
    }
}

/// Reads a one-byte flag reply such as `CaseLidState` or `ChargingState`.
pub fn flag(frame: &Frame) -> Result<bool, Malformed> {
    match frame.payload[..] {
        [0] => Ok(false),
        [1] => Ok(true),
        _ => Err(frame.malformed("one byte, 0 or 1")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured(hex: &str) -> Frame {
        hex.parse().unwrap()
    }

    #[test]
    fn battery_level_reads_case_then_left_then_right() {
        let expected = BatteryLevel {
            case: 100,
            left: 96,
            right: 95,
        };
        let frame = captured("02 02 03 06 64 60 5f 00 00 00");
        assert_eq!(BatteryLevel::parse(&frame), Ok(expected));
    }

    #[test]
    fn battery_level_rejects_a_short_payload() {
        assert!(BatteryLevel::parse(&captured("02 02 03 02 64 60")).is_err());
    }

    #[test]
    fn buds_location_reads_both_out_of_case() {
        let frame = captured("02 08 03 01 01");
        assert_eq!(BudsLocation::parse(&frame), Ok(BudsLocation::OutOfCase));
    }

    #[test]
    fn buds_connection_reads_flag_then_rssi_per_bud() {
        let frame = captured("02 09 03 06 01 ff f0 01 ff e4");
        let expected = BudsConnection {
            left: Some(-16),
            right: Some(-28),
        };
        assert_eq!(BudsConnection::parse(&frame), Ok(expected));
    }

    #[test]
    fn buds_connection_drops_rssi_of_an_unlinked_bud() {
        let frame = captured("02 09 03 06 00 ff 81 01 ff e4");
        assert_eq!(BudsConnection::parse(&frame).unwrap().left, None);
    }

    #[test]
    fn flag_reads_lid_open() {
        assert_eq!(flag(&captured("02 0f 03 01 01")), Ok(true));
        assert!(flag(&captured("02 0f 03 01 07")).is_err());
    }
}
