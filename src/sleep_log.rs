//! The record format of the sleep and sensor logs.
//!
//! A log is a run of records, each `[category, length, type, timestamp,
//! payload]`. `length` counts the type, the timestamp and the payload. The
//! device stores logs in flash pages and pads each page's tail with filler:
//! `fd` alone, or `fe`, a count, and that many `ff` bytes.

use std::fmt;

const PAD_ONE: u8 = 0xfd;
const PAD_RUN: u8 = 0xfe;
/// Type, plus the four timestamp bytes.
const FIXED_BYTES: usize = 5;

/// One entry of a log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// The stream the record belongs to: 0 for sensor samples, others for events.
    pub category: u8,
    /// The record type within its category.
    pub kind: u8,
    /// Seconds since 1970 on the device's clock, which keeps local wall time.
    pub timestamp: u32,
    pub payload: Vec<u8>,
}

/// A place where the bytes stop following the record format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corrupt {
    pub offset: usize,
}

impl fmt::Display for Corrupt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the log stops making sense at byte {}", self.offset)
    }
}

impl std::error::Error for Corrupt {}

/// Splits a downloaded log into its records.
///
/// A record cut short by the end of the data is dropped, because the device
/// keeps appending while the log is read.
///
/// # Errors
///
/// Returns [`Corrupt`] if a record declares a length too short to hold a
/// timestamp.
pub fn records(log: &[u8]) -> Result<Vec<Record>, Corrupt> {
    let mut records = Vec::new();
    let mut offset = 0;
    while offset < log.len() {
        match log[offset..] {
            [PAD_ONE, ..] => offset += 1,
            [PAD_RUN, count, ..] => offset += 2 + usize::from(count),
            [category, length, kind, t0, t1, t2, t3, ..] => {
                let length = usize::from(length);
                if length < FIXED_BYTES {
                    return Err(Corrupt { offset });
                }
                let end = offset + 2 + length;
                let Some(payload) = log.get(offset + 2 + FIXED_BYTES..end) else {
                    break;
                };
                records.push(Record {
                    category,
                    kind,
                    timestamp: u32::from_be_bytes([t0, t1, t2, t3]),
                    payload: payload.to_vec(),
                });
                offset = end;
            }
            _ => break,
        }
    }
    Ok(records)
}

/// What a record's payload means, as far as it is understood.
///
/// Sensor records are run-length encoded: a reading and how long it held.
/// Event records start with the recorder's battery level.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Meaning {
    /// A short name for the record type, or empty if it is not understood.
    pub name: &'static str,
    /// How long a sensor reading held, in seconds.
    pub held_seconds: Option<u32>,
    /// The recorder's battery level in percent, for event records.
    pub battery: Option<u8>,
    /// Decoded fields as `key=value` pairs separated by spaces.
    pub values: String,
}

const CASE_SAMPLE_SECONDS: u32 = 30;
const BUD_SAMPLE_SECONDS: u32 = 1;

impl Record {
    /// Decodes the payload of the record types understood so far.
    ///
    /// Unknown types and payloads of an unexpected length come back unnamed,
    /// with only the run length or battery level filled in.
    pub fn meaning(&self) -> Meaning {
        if self.category == 0 {
            return self.sensor_reading();
        }
        let [battery, fields @ ..] = &self.payload[..] else {
            return Meaning::default();
        };
        let (name, values) = match (self.category, self.kind, fields) {
            (1, 4, [_, playing, sound, volume]) | (2 | 3, 2, [playing, sound, volume]) => (
                "playback",
                format!("playing={playing} sound={sound} volume={volume}"),
            ),
            (1, 5, [volume]) | (2 | 3, 3, [volume]) => ("volume", format!("volume={volume}")),
            (1, 15, [_, high, low, left, right]) => (
                "battery",
                format!(
                    "case_mv={} left={left} right={right}",
                    u16::from_be_bytes([*high, *low])
                ),
            ),
            (2 | 3, 7, [_, high, low]) => (
                "battery",
                format!("mv={}", u16::from_be_bytes([*high, *low])),
            ),
            (2 | 3, 0, [_, _, _, version @ ..]) => (
                "firmware",
                format!("version={}", String::from_utf8_lossy(version)),
            ),
            _ => ("", String::new()),
        };
        // Usage records of type 18 are alarm slots and carry no battery byte.
        if let (4, 18, [index, hour, minute, enabled, volume, days, sound]) =
            (self.category, self.kind, &self.payload[..])
        {
            return Meaning {
                name: "alarm",
                values: format!(
                    "index={index} time={hour:02}:{minute:02} enabled={enabled} volume={volume} days={days:#04x} sound={sound}"
                ),
                ..Meaning::default()
            };
        }
        Meaning {
            name,
            battery: Some(*battery),
            values,
            ..Meaning::default()
        }
    }

    fn sensor_reading(&self) -> Meaning {
        let [count_high, count_low, reading @ ..] = &self.payload[..] else {
            return Meaning::default();
        };
        let periods = u32::from(u16::from_be_bytes([*count_high, *count_low]));
        let (name, period, values) = match (self.kind, reading) {
            (21, [high, low]) => (
                "temperature",
                CASE_SAMPLE_SECONDS,
                format!(
                    "celsius={:.1}",
                    f32::from(u16::from_be_bytes([*high, *low])) / 10.0
                ),
            ),
            (20, [a, b, c, d]) => (
                "pressure",
                CASE_SAMPLE_SECONDS,
                format!(
                    "hpa={:.2}",
                    f64::from(u32::from_be_bytes([*a, *b, *c, *d])) / 100.0
                ),
            ),
            (22, [first, second]) => (
                "noise",
                CASE_SAMPLE_SECONDS,
                format!("first={first} second={second}"),
            ),
            (18, [a, b, c, d]) => (
                "light",
                CASE_SAMPLE_SECONDS,
                format!(
                    "first={} second={}",
                    u16::from_be_bytes([*a, *b]),
                    u16::from_be_bytes([*c, *d])
                ),
            ),
            (18 | 20 | 21 | 22, _) => ("", CASE_SAMPLE_SECONDS, String::new()),
            (_, [value]) => ("", BUD_SAMPLE_SECONDS, format!("value={value}")),
            (_, [high, low, extra]) => (
                "",
                BUD_SAMPLE_SECONDS,
                format!("value={} extra={extra}", u16::from_be_bytes([*high, *low])),
            ),
            _ => ("", BUD_SAMPLE_SECONDS, String::new()),
        };
        Meaning {
            name,
            held_seconds: Some(periods * period),
            values,
            ..Meaning::default()
        }
    }
}

/// Renders records as CSV with a header row, one record per line.
///
/// Times are the device's local wall time. Payloads are spaced hex.
pub fn csv(recorder: &str, records: &[Record]) -> String {
    let mut table =
        String::from("recorder,time,category,type,name,held_seconds,battery,values,payload\n");
    for record in records {
        let time = jiff::Timestamp::from_second(i64::from(record.timestamp))
            .map(|time| time.strftime("%Y-%m-%dT%H:%M:%S").to_string())
            .unwrap_or_default();
        let payload: Vec<String> = record
            .payload
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let meaning = record.meaning();
        let shown = |number: Option<u32>| number.map(|n| n.to_string()).unwrap_or_default();
        table.push_str(&format!(
            "{recorder},{time},{},{},{},{},{},{},{}\n",
            record.category,
            record.kind,
            meaning.name,
            shown(meaning.held_seconds),
            shown(meaning.battery.map(u32::from)),
            meaning.values,
            payload.join(" ")
        ));
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    // The first two records of a captured case log.
    const CASE_LOG_START: [u8; 28] = [
        0x04, 0x0f, 0x09, 0x6b, 0x08, 0xd1, 0xb5, 0x64, 0x00, 0x00, 0x00, 0x12, 0x00, 0x00, 0x00,
        0x02, 0x07, 0x01, 0x09, 0x02, 0x6b, 0x08, 0xd1, 0xbd, 0x64, 0x00, 0x00, 0x3d,
    ];

    #[test]
    fn records_split_on_the_length_byte() {
        let records = records(&CASE_LOG_START).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!((records[0].category, records[0].kind), (4, 9));
        assert_eq!(records[0].timestamp, 0x6b08_d1b5);
        assert_eq!(records[0].payload.len(), 10);
        assert_eq!((records[1].category, records[1].kind), (1, 2));
        assert_eq!(records[1].payload, [0x64, 0x00, 0x00, 0x3d]);
    }

    #[test]
    fn page_filler_is_skipped() {
        let record = &CASE_LOG_START[17..];
        let mut log = record.to_vec();
        log.extend([0xfe, 0x03, 0xff, 0xff, 0xff]);
        log.extend(record);
        log.push(0xfd);
        log.extend([0xfe, 0x00]);
        log.extend(record);
        assert_eq!(records(&log).unwrap().len(), 3);
    }

    #[test]
    fn a_record_cut_off_by_the_end_is_dropped() {
        let cut = &CASE_LOG_START[..CASE_LOG_START.len() - 2];
        assert_eq!(records(cut).unwrap().len(), 1);
    }

    #[test]
    fn a_length_too_short_for_a_timestamp_is_corrupt() {
        let log = [0x04, 0x02, 0x09, 0x6b, 0x08, 0xd1, 0xb5, 0x64];
        assert_eq!(records(&log), Err(Corrupt { offset: 0 }));
    }

    #[test]
    fn csv_writes_local_wall_time_and_hex() {
        let table = csv("case", &records(&CASE_LOG_START).unwrap());
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(
            lines[0],
            "recorder,time,category,type,name,held_seconds,battery,values,payload"
        );
        assert_eq!(lines[2], "case,2026-11-27T00:41:33,1,2,,,100,,64 00 00 3d");
    }

    fn captured(category: u8, kind: u8, payload: &[u8]) -> Meaning {
        let record = Record {
            category,
            kind,
            timestamp: 0x6b08_d1b5,
            payload: payload.to_vec(),
        };
        record.meaning()
    }

    #[test]
    fn case_sensor_readings_carry_a_run_length_in_half_minutes() {
        // Temperature held for 3 periods: the next record came 90 s later.
        let temperature = captured(0, 21, &[0x00, 0x03, 0x00, 0xde]);
        assert_eq!(temperature.name, "temperature");
        assert_eq!(temperature.held_seconds, Some(90));
        assert_eq!(temperature.values, "celsius=22.2");

        let pressure = captured(0, 20, &[0x00, 0x01, 0x00, 0x01, 0x82, 0x5e]);
        assert_eq!(pressure.values, "hpa=989.10");

        let noise = captured(0, 22, &[0x00, 0x01, 0x21, 0x5e]);
        assert_eq!(
            (noise.name, noise.values.as_str()),
            ("noise", "first=33 second=94")
        );

        let light = captured(0, 18, &[0x00, 0x01, 0x00, 0x6b, 0x00, 0xf3]);
        assert_eq!(light.values, "first=107 second=243");
    }

    #[test]
    fn bud_sensor_readings_run_in_seconds_and_stay_unnamed() {
        // Held for 0x59 periods: the next record came 89 s later.
        let reading = captured(0, 8, &[0x00, 0x59, 0x64]);
        assert_eq!(reading.name, "");
        assert_eq!(reading.held_seconds, Some(89));
        assert_eq!(reading.values, "value=100");
        let wide = captured(0, 3, &[0x00, 0x01, 0x06, 0xa3, 0x00]);
        assert_eq!(wide.values, "value=1699 extra=0");
    }

    #[test]
    fn events_start_with_the_battery_level() {
        let case_playback = captured(1, 4, &[0x64, 0x00, 0x01, 0x12, 0x28]);
        assert_eq!(case_playback.name, "playback");
        assert_eq!(case_playback.battery, Some(100));
        assert_eq!(case_playback.values, "playing=1 sound=18 volume=40");

        let bud_playback = captured(2, 2, &[0x63, 0x01, 0x12, 0x28]);
        assert_eq!(bud_playback.values, "playing=1 sound=18 volume=40");
        assert_eq!(bud_playback.battery, Some(99));

        assert_eq!(captured(1, 5, &[0x64, 0x26]).values, "volume=38");
        let battery = captured(1, 15, &[0x64, 0x02, 0x10, 0xaf, 0x63, 0x63]);
        assert_eq!(battery.values, "case_mv=4271 left=99 right=99");
        assert_eq!(captured(3, 7, &[0x63, 0x00, 0x10, 0xdb]).values, "mv=4315");
    }

    #[test]
    fn alarm_slots_and_firmware_decode() {
        let alarm = captured(4, 18, &[0x00, 0x06, 0x2d, 0x01, 0x64, 0x3e, 0x08]);
        assert_eq!(alarm.name, "alarm");
        assert_eq!(alarm.battery, None);
        assert_eq!(
            alarm.values,
            "index=0 time=06:45 enabled=1 volume=100 days=0x3e sound=8"
        );
        let firmware = captured(2, 0, b"\x63\x00\x00\x0702.11.2606011");
        assert_eq!(firmware.values, "version=02.11.2606011");
    }

    #[test]
    fn an_unknown_event_keeps_only_its_battery_level() {
        let unknown = captured(4, 13, &[0x64, 0x01, 0x0e]);
        assert_eq!(unknown.name, "");
        assert_eq!(unknown.battery, Some(100));
        assert_eq!(unknown.values, "");
    }
}
