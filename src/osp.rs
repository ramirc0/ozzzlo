//! Ozlo Sleepbuds Protocol (OSP) frames.
//!
//! A frame is `[block, function, operator, length, payload...]`. The same
//! frame travels over USB HID and BLE. Each transport adds its own envelope.

use std::fmt;
use std::str::FromStr;

pub mod audio;
pub mod data;
pub mod product_info;
pub mod settings;
pub mod status;

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

const BLOCK_NAMES: &[(u8, &str)] = &[
    (0, "ProductInfo"),
    (1, "Settings"),
    (2, "Status"),
    (3, "FirmwareUpdate"),
    (4, "DeviceManagement"),
    (5, "AudioManagement"),
    (8, "Debug"),
    (9, "Notifications"),
    (13, "DataCollection"),
    (17, "ManufacturingTest"),
];

const FUNCTION_NAMES: &[(u8, u8, &str)] = &[
    (0, 0, "BlockVersion"),
    (0, 1, "OSPVersion"),
    (0, 2, "GetAllFunctionBlocks"),
    (0, 3, "ProductVariant"),
    (0, 4, "GetAll"),
    (0, 5, "FirmwareVersion"),
    (0, 6, "MacAddress"),
    (0, 7, "SerialNumber"),
    (0, 8, "DateOfManufacture"),
    (0, 9, "BoardSerialNumber"),
    (0, 10, "HardwareRevision"),
    (0, 11, "ComponentDevices"),
    (0, 14, "DeviceFeaturesMask"),
    (0, 15, "ProductAssemblyNumber"),
    (1, 0, "BlockVersion"),
    (1, 1, "GetAll"),
    (1, 2, "ProductName"),
    (1, 6, "Alarms"),
    (1, 7, "SoundLoopList"),
    (1, 8, "CurrentSoundLoop"),
    (1, 9, "ButtonFunction"),
    (1, 10, "AlarmsSnooze"),
    (1, 11, "PhoneFreeMode"),
    (1, 12, "FavoriteLoopsList"),
    (1, 13, "SleepTimer"),
    (1, 14, "CurrentTime"),
    (1, 16, "AutoStart"),
    (1, 17, "AutoStartAfterSnooze"),
    (1, 18, "AlarmSoundList"),
    (1, 19, "DeleteAlarm"),
    (1, 20, "DeviceIsConfigured"),
    (1, 21, "StopAlarm"),
    (1, 22, "CurrentDateTime"),
    (1, 23, "BluetoothMode"),
    (1, 24, "AppleNotifications"),
    (1, 25, "NotificationAlertSound"),
    (1, 26, "SleepModelSensitivity"),
    (1, 27, "Timezone"),
    (1, 28, "SleepSchedule"),
    (1, 29, "SleepGoals"),
    (1, 30, "DoNotDisturbMode"),
    (1, 31, "NotificationContactInfo"),
    (1, 32, "NotificationFilter"),
    (2, 0, "BlockVersion"),
    (2, 1, "GetAll"),
    (2, 2, "BatteryLevel"),
    (2, 4, "MicLevel"),
    (2, 5, "ChargingState"),
    (2, 8, "BudsLocation"),
    (2, 9, "BudsConnectionState"),
    (2, 10, "CurrentAlarmState"),
    (2, 11, "HealthDataDates"),
    (2, 12, "HealthMetadata"),
    (2, 13, "HealthDataTransfer"),
    (2, 14, "HealthDataValidate"),
    (2, 15, "CaseLidState"),
    (2, 16, "DeviceRssi"),
    (2, 18, "DeviceUsageStatistics"),
    (3, 12, "UpdateProgress"),
    (4, 0, "BlockVersion"),
    (4, 1, "Connect"),
    (4, 2, "Disconnect"),
    (4, 3, "Remove"),
    (4, 4, "DeviceList"),
    (4, 7, "ClearDeviceList"),
    (4, 8, "PairingMode"),
    (4, 9, "AppAddress"),
    (4, 13, "SearchingForBuds"),
    (4, 14, "ConnectingToBuds"),
    (4, 15, "FactoryReset"),
    (4, 16, "ResetDevice"),
    (4, 17, "AppFeatures"),
    (4, 18, "CaseBudBtAddress"),
    (4, 19, "AppScratchpad"),
    (4, 20, "PairDockedBud"),
    (4, 21, "UpdateSiblingId"),
    (4, 22, "LedIntensity"),
    (5, 0, "BlockVersion"),
    (5, 2, "GetAll"),
    (5, 3, "Control"),
    (5, 4, "Status"),
    (5, 5, "Volume"),
    (5, 6, "NowPlaying"),
    (5, 7, "PlayingLoop"),
    (5, 8, "AlarmPlayingWithVolume"),
    (5, 9, "SoundLoopVolume"),
    (5, 10, "ResetSoundId"),
    (5, 11, "SoundInfo"),
    (5, 29, "SoundAddInit"),
    (5, 30, "SoundTransfer"),
    (5, 31, "SetSound"),
    (5, 33, "ConnectingToneVolume"),
    (5, 34, "SoundIntegrityCheck"),
    (5, 35, "SoundDelete"),
    (5, 36, "SoundReplace"),
    (5, 37, "SoundUpdateSession"),
    (5, 38, "NotificationAlert"),
    (5, 40, "SoundQuickCheck"),
    (5, 47, "DefaultSoundId"),
    (5, 48, "VolumeBalance"),
    (13, 1, "Clear"),
    (13, 2, "ReadInit"),
    (13, 3, "ReadTransfer"),
    (13, 4, "Storing"),
    (13, 5, "Size"),
    (13, 6, "SensorConfig"),
];

impl Function {
    /// Whether sending to this function could brick the case, erase user
    /// data, or break its pairing with the buds or the host.
    pub fn is_hazardous(self) -> bool {
        matches!(
            (self.block, self.id),
            (3 | 17, _)
                | (4, 3 | 7 | 15 | 16 | 18 | 20 | 21)
                | (5, 10 | 29 | 30 | 35 | 36 | 37)
                | (13, 1)
        )
    }
}

impl fmt::Display for Function {
    /// Writes `Block.Function` by name where known, and by number otherwise.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match BLOCK_NAMES.iter().find(|(block, _)| *block == self.block) {
            Some((_, name)) => write!(f, "{name}.")?,
            None => write!(f, "Block{}.", self.block)?,
        }
        let known = FUNCTION_NAMES
            .iter()
            .find(|(block, id, _)| *block == self.block && *id == self.id);
        match known {
            Some((_, _, name)) => write!(f, "{name}"),
            None => write!(f, "Function{}", self.id),
        }
    }
}

/// The code the case sends in an `Error` reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorCode(pub u8);

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let meaning = match self.0 {
            1 => "invalid length",
            2 => "invalid checksum",
            3 => "function block not supported",
            4 => "function not supported",
            5 => "operator not supported for this function",
            6 => "sent values are incorrect",
            7 => "requested data is not available",
            8 => "temporary read or write error",
            9 => "operation timed out",
            10 => "not applicable in the current state",
            12 => "device is busy",
            20 => "buds must be in the case",
            21 => "buds are not connected",
            22 => "input values are invalid",
            29 => "buds are not docked in the case",
            255 => "function block specific error",
            _ => "unknown error",
        };
        write!(f, "{} ({meaning})", self.0)
    }
}

/// A payload that does not match the layout its function is known to use.
///
/// A firmware update that changes a layout surfaces as this error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Malformed {
    pub function: Function,
    pub expected: &'static str,
    pub payload: Vec<u8>,
}

impl fmt::Display for Malformed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: expected {}, got {} bytes [{}]",
            self.function,
            self.expected,
            self.payload.len(),
            spaced_hex(&self.payload)
        )
    }
}

impl std::error::Error for Malformed {}

fn spaced_hex(bytes: &[u8]) -> String {
    let hex: Vec<String> = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    hex.join(" ")
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
    /// A token in a hex string is not one byte of hex.
    NotHex(String),
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => write!(f, "frame is shorter than its header or length field"),
            Self::UnknownOperator(byte) => write!(f, "unknown operator byte {byte:#04x}"),
            Self::PayloadTooLong(len) => {
                write!(f, "payload of {len} bytes exceeds {MAX_PAYLOAD}")
            }
            Self::NotHex(token) => write!(f, "`{token}` is not a hex byte"),
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

    /// A request that starts the action `function` names, with no arguments.
    pub fn start(function: Function) -> Self {
        Self {
            function,
            operator: Operator::Start,
            payload: Vec::new(),
        }
    }

    /// A request that writes `payload` to `function` and asks for the new value.
    pub fn set_get(function: Function, payload: Vec<u8>) -> Self {
        Self {
            function,
            operator: Operator::SetGet,
            payload,
        }
    }

    /// Whether this frame settles `request`.
    ///
    /// `Processing` only announces that more frames follow, so it does not.
    pub fn answers(&self, request: &Frame) -> bool {
        self.function == request.function
            && matches!(
                self.operator,
                Operator::Status | Operator::Error | Operator::Result
            )
    }

    /// The error code, if this frame is an `Error` reply.
    pub fn refusal(&self) -> Option<ErrorCode> {
        (self.operator == Operator::Error)
            .then(|| ErrorCode(self.payload.first().copied().unwrap_or(0)))
    }

    /// Builds the error for a payload that does not fit `expected`.
    pub fn malformed(&self, expected: &'static str) -> Malformed {
        Malformed {
            function: self.function,
            expected,
            payload: self.payload.clone(),
        }
    }

    /// Serializes the frame as `[block, function, operator, length, payload...]`.
    ///
    /// Fails if the payload exceeds [`MAX_PAYLOAD`].
    ///
    /// # Errors
    ///
    /// Returns [`FrameError::PayloadTooLong`] if the payload exceeds [`MAX_PAYLOAD`].
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
    ///
    /// # Errors
    ///
    /// Returns [`FrameError::Truncated`] if bytes are missing, and
    /// [`FrameError::UnknownOperator`] if the operator is not a known one.
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

impl fmt::Display for Frame {
    /// Writes the encoded frame as spaced hex, the form [`Frame::from_str`] reads.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut header = vec![self.function.block, self.function.id, self.operator as u8];
        header.push(self.payload.len() as u8);
        write!(f, "{}", spaced_hex(&header))?;
        if !self.payload.is_empty() {
            write!(f, " {}", spaced_hex(&self.payload))?;
        }
        Ok(())
    }
}

impl FromStr for Frame {
    type Err = FrameError;

    /// Parses spaced hex such as `02 02 01 00`.
    fn from_str(hex: &str) -> Result<Self, FrameError> {
        let bytes: Result<Vec<u8>, FrameError> = hex
            .split_whitespace()
            .map(|token| {
                u8::from_str_radix(token, 16).map_err(|_| FrameError::NotHex(token.to_string()))
            })
            .collect();
        Self::decode(&bytes?)
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
        assert_eq!(frame.function, product_info::OSP_VERSION);
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
    fn hex_round_trips_through_display_and_from_str() {
        let frame: Frame = "02 02 03 06 64 60 60 00 00 00".parse().unwrap();
        assert_eq!(frame.function, status::BATTERY_LEVEL);
        assert_eq!(frame.to_string(), "02 02 03 06 64 60 60 00 00 00");
        assert_eq!(
            "02 zz".parse::<Frame>(),
            Err(FrameError::NotHex("zz".to_string()))
        );
    }

    #[test]
    fn names_known_functions_and_numbers_unknown_ones() {
        assert_eq!(status::BATTERY_LEVEL.to_string(), "Status.BatteryLevel");
        assert_eq!(
            Function { block: 42, id: 7 }.to_string(),
            "Block42.Function7"
        );
    }

    #[test]
    fn processing_does_not_answer_but_result_does() {
        let request = Frame::start(status::GET_ALL);
        let processing: Frame = "02 01 07 00".parse().unwrap();
        let result: Frame = "02 01 06 00".parse().unwrap();
        let other: Frame = "02 02 03 03 64 60 60".parse().unwrap();
        assert!(!processing.answers(&request));
        assert!(result.answers(&request));
        assert!(!other.answers(&request));
    }

    #[test]
    fn every_destructive_function_is_hazardous() {
        for id in 0..=255 {
            assert!(Function { block: 3, id }.is_hazardous());
            assert!(Function { block: 17, id }.is_hazardous());
        }
        for id in [3, 7, 15, 16, 18, 20, 21] {
            assert!(Function { block: 4, id }.is_hazardous());
        }
        for id in [10, 29, 30, 35, 36, 37] {
            assert!(Function { block: 5, id }.is_hazardous());
        }
        assert!(Function { block: 13, id: 1 }.is_hazardous());
        assert!(!status::BATTERY_LEVEL.is_hazardous());
        assert!(!Function { block: 4, id: 8 }.is_hazardous());
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
