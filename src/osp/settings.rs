//! The Settings block: persistent preferences and alarms.

use std::fmt;

use super::{Frame, Function, Malformed};

pub const GET_ALL: Function = Function { block: 1, id: 1 };
pub const PRODUCT_NAME: Function = Function { block: 1, id: 2 };
pub const ALARMS: Function = Function { block: 1, id: 6 };
pub const SOUND_LOOP_LIST: Function = Function { block: 1, id: 7 };
pub const CURRENT_SOUND_LOOP: Function = Function { block: 1, id: 8 };
pub const SLEEP_TIMER: Function = Function { block: 1, id: 13 };
pub const AUTO_START: Function = Function { block: 1, id: 16 };
pub const ALARM_SOUND_LIST: Function = Function { block: 1, id: 18 };
pub const CURRENT_DATE_TIME: Function = Function { block: 1, id: 22 };
pub const BLUETOOTH_MODE: Function = Function { block: 1, id: 23 };

/// A sound stored on the buds, as the case lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sound {
    pub id: u8,
    /// The case's own label, such as `M18`.
    pub label: String,
}

impl Sound {
    /// Reads a `SoundLoopList` reply: a count, then ID, label and kind per sound.
    pub fn parse_loops(frame: &Frame) -> Result<Vec<Self>, Malformed> {
        Self::parse_list(frame, 1)
    }

    /// Reads an `AlarmSoundList` reply: a count, then ID and label per sound.
    pub fn parse_alarm_sounds(frame: &Frame) -> Result<Vec<Self>, Malformed> {
        Self::parse_list(frame, 0)
    }

    fn parse_list(frame: &Frame, trailing_bytes: usize) -> Result<Vec<Self>, Malformed> {
        let expected = "a count, then ID, label length and label per sound";
        let Some((_count, mut rest)) = frame.payload.split_first() else {
            return Err(frame.malformed(expected));
        };
        let mut sounds = Vec::new();
        while let [id, length, tail @ ..] = rest {
            let label_end = usize::from(*length);
            let entry_end = label_end + trailing_bytes;
            if tail.len() < entry_end {
                return Err(frame.malformed(expected));
            }
            sounds.push(Self {
                id: *id,
                label: String::from_utf8_lossy(&tail[..label_end]).to_string(),
            });
            rest = &tail[entry_end..];
        }
        if rest.is_empty() {
            Ok(sounds)
        } else {
            Err(frame.malformed(expected))
        }
    }
}

/// The marketing name of a built-in sound loop.
pub fn loop_title(id: u8) -> Option<&'static str> {
    Some(match id {
        0 => "Babbling Brook",
        1 => "Crimson Cloak",
        2 => "Fireglow",
        3 => "Jade Static",
        4 => "Misty Rain",
        5 => "Ocean Tide",
        6 => "Rainfall",
        7 => "Soar",
        8 => "Stratus",
        9 => "Tumbling Waters",
        10 => "Warm Glow",
        11 => "Winter White",
        12 => "Pink Haze",
        13 => "Green Dream",
        14 => "Brown Noise",
        15 => "Oh So Gray",
        16 => "Into the Woods",
        17 => "Summer Nights",
        18 => "Intimate Rain",
        19 => "Gentle Ocean",
        20 => "Air Waves",
        21 => "Rolling Thunder",
        22 => "Interstate",
        23 => "Beach Walk",
        24 => "Beneath the Canopy",
        25 => "All Aboard",
        26 => "By the Bay",
        28 => "Autumn",
        29 => "Subtle Waves",
        30 => "Outboard",
        31 => "Rainy Nook",
        32 => "Base Camp",
        37 => "Mountain Air",
        41 => "Emberwell",
        42 => "Steady Heat",
        _ => return None,
    })
}

/// The marketing name of a built-in alarm sound.
pub fn alarm_sound_title(id: u8) -> Option<&'static str> {
    Some(match id {
        0 => "Bubble Bells",
        1 => "Cosmic Chime",
        2 => "Peppermint Piano",
        3 => "Business Time",
        4 => "Gilded Throne",
        5 => "Marimba March",
        6 => "Space Dust",
        8 => "Wake Up",
        _ => return None,
    })
}

/// The weekdays an alarm repeats on. Bit 0 is Sunday.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Days(pub u8);

impl fmt::Display for Days {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        const NAMES: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        match self.0 & 0x7f {
            0 => write!(f, "once"),
            0x7f => write!(f, "every day"),
            0x3e => write!(f, "weekdays"),
            0x41 => write!(f, "weekends"),
            mask => {
                let chosen: Vec<&str> = (0..7)
                    .filter(|day| mask & (1 << day) != 0)
                    .map(|day| NAMES[day])
                    .collect();
                write!(f, "{}", chosen.join(" "))
            }
        }
    }
}

/// One alarm slot on the case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alarm {
    pub index: u8,
    pub hour: u8,
    pub minute: u8,
    pub enabled: bool,
    pub volume: u8,
    pub days: Days,
    pub sound: u8,
}

impl Alarm {
    /// Reads an `Alarms` reply: a count, then one 7-byte record per alarm.
    pub fn parse_all(frame: &Frame) -> Result<Vec<Self>, Malformed> {
        let expected = "a count, then 7-byte alarm records";
        let [_count, records @ ..] = &frame.payload[..] else {
            return Err(frame.malformed(expected));
        };
        if records.len() % 7 != 0 {
            return Err(frame.malformed(expected));
        }
        Ok(records
            .chunks_exact(7)
            .map(|record| Self {
                index: record[0],
                hour: record[1],
                minute: record[2],
                enabled: record[3] == 1,
                volume: record[4],
                days: Days(record[5]),
                sound: record[6],
            })
            .collect())
    }
}

/// When a sound starts once the buds leave the case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoStart {
    Immediately,
    Off,
    WhenAsleep,
    AfterMinutes(u8),
}

impl AutoStart {
    /// Reads an `AutoStart` reply: a mode, plus minutes for the delay mode.
    pub fn parse(frame: &Frame) -> Result<Self, Malformed> {
        match frame.payload[..] {
            [0] => Ok(Self::Immediately),
            [1] => Ok(Self::Off),
            [2] => Ok(Self::WhenAsleep),
            [3, minutes] => Ok(Self::AfterMinutes(minutes)),
            _ => Err(frame.malformed("a mode from 0 to 3, with minutes after mode 3")),
        }
    }
}

/// Whether the case accepts streamed Bluetooth audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BluetoothMode {
    Disabled,
    Enabled,
    PowerSaving,
}

impl BluetoothMode {
    /// Reads a `BluetoothMode` reply: a mode, then a 16-bit time remaining.
    pub fn parse(frame: &Frame) -> Result<Self, Malformed> {
        match frame.payload[..] {
            [0, ..] => Ok(Self::Disabled),
            [1, ..] => Ok(Self::Enabled),
            [2, ..] => Ok(Self::PowerSaving),
            _ => Err(frame.malformed("a mode from 0 to 2")),
        }
    }
}

/// The case's wall clock, which its alarms fire by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaseClock {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl CaseClock {
    /// Reads a `CurrentDateTime` reply: year since 2000, month, day, weekday,
    /// hour, minute, second.
    pub fn parse(frame: &Frame) -> Result<Self, Malformed> {
        let [year, month, day, _weekday, hour, minute, second] = frame.payload[..] else {
            return Err(frame.malformed("seven date and time bytes"));
        };
        Ok(Self {
            year: 2000 + u16::from(year),
            month,
            day,
            hour,
            minute,
            second,
        })
    }
}

/// Reads a `SleepTimer` reply in minutes. Zero means the timer is off.
pub fn sleep_timer_minutes(frame: &Frame) -> Result<u16, Malformed> {
    match frame.payload[..] {
        [high, low] => Ok(u16::from_be_bytes([high, low])),
        _ => Err(frame.malformed("16-bit minutes")),
    }
}

/// Reads a `CurrentSoundLoop` reply.
pub fn current_loop(frame: &Frame) -> Result<u8, Malformed> {
    match frame.payload[..] {
        [id] => Ok(id),
        _ => Err(frame.malformed("one loop ID byte")),
    }
}

/// Reads a `ProductName` reply: a flag byte, then the name.
pub fn product_name(frame: &Frame) -> Result<String, Malformed> {
    match &frame.payload[..] {
        [_flag, name @ ..] => Ok(String::from_utf8_lossy(name).to_string()),
        _ => Err(frame.malformed("a flag byte, then a name")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured(hex: &str) -> Frame {
        hex.parse().unwrap()
    }

    #[test]
    fn sound_loops_carry_id_label_and_a_kind_byte() {
        let frame = captured("01 07 03 0c 02 00 02 4d 30 01 12 03 4d 31 38 01");
        let sounds = Sound::parse_loops(&frame).unwrap();
        assert_eq!(sounds.len(), 2);
        assert_eq!((sounds[0].id, sounds[0].label.as_str()), (0, "M0"));
        assert_eq!((sounds[1].id, sounds[1].label.as_str()), (18, "M18"));
    }

    #[test]
    fn alarm_sounds_have_no_kind_byte() {
        let frame = captured("01 12 03 0d 03 08 02 41 38 04 02 41 34 09 02 41 39");
        let sounds = Sound::parse_alarm_sounds(&frame).unwrap();
        let ids: Vec<u8> = sounds.iter().map(|sound| sound.id).collect();
        assert_eq!(ids, [8, 4, 9]);
        assert_eq!(sounds[0].label, "A8");
    }

    #[test]
    fn sound_list_rejects_a_truncated_entry() {
        assert!(Sound::parse_loops(&captured("01 07 03 04 01 00 02 4d")).is_err());
    }

    #[test]
    fn alarms_read_seven_byte_records() {
        let frame = captured("01 06 03 0f 02 00 06 2d 01 64 3e 08 07 09 00 00 64 41 08");
        let alarms = Alarm::parse_all(&frame).unwrap();
        assert_eq!(alarms.len(), 2);
        let first = alarms[0];
        assert_eq!((first.index, first.hour, first.minute), (0, 6, 45));
        assert!(first.enabled);
        assert_eq!((first.volume, first.sound), (100, 8));
        assert_eq!(first.days.to_string(), "weekdays");
        assert!(!alarms[1].enabled);
        assert_eq!(alarms[1].days.to_string(), "weekends");
    }

    #[test]
    fn alarms_reject_a_partial_record() {
        assert!(Alarm::parse_all(&captured("01 06 03 04 01 00 06 2d")).is_err());
    }

    #[test]
    fn days_name_individual_weekdays() {
        assert_eq!(Days(0b0010_1010).to_string(), "Mon Wed Fri");
        assert_eq!(Days(0).to_string(), "once");
        assert_eq!(Days(0x7f).to_string(), "every day");
    }

    #[test]
    fn clock_reads_date_then_weekday_then_time() {
        let clock = CaseClock::parse(&captured("01 16 03 07 1a 0b 1b 05 02 23 0e")).unwrap();
        assert_eq!((clock.year, clock.month, clock.day), (2026, 11, 27));
        assert_eq!((clock.hour, clock.minute, clock.second), (2, 35, 14));
    }

    #[test]
    fn simple_settings_read_their_captured_replies() {
        assert_eq!(sleep_timer_minutes(&captured("01 0d 03 02 00 00")), Ok(0));
        assert_eq!(current_loop(&captured("01 08 03 01 12")), Ok(18));
        assert_eq!(
            AutoStart::parse(&captured("01 10 03 01 00")),
            Ok(AutoStart::Immediately)
        );
        assert_eq!(
            AutoStart::parse(&captured("01 10 03 02 03 2d")),
            Ok(AutoStart::AfterMinutes(45))
        );
        assert_eq!(
            BluetoothMode::parse(&captured("01 17 03 03 01 00 00")),
            Ok(BluetoothMode::Enabled)
        );
        let name = captured("01 02 03 11 00 4f 7a 6c 6f 20 53 6c 65 65 70 62 75 64 73 20 32");
        assert_eq!(product_name(&name), Ok("Ozlo Sleepbuds 2".to_string()));
    }
}
