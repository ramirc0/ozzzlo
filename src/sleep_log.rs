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

/// Renders records as CSV with a header row, one record per line.
///
/// Times are the device's local wall time. Payloads are spaced hex.
pub fn csv(recorder: &str, records: &[Record]) -> String {
    let mut table = String::from("recorder,time,category,type,payload\n");
    for record in records {
        let time = jiff::Timestamp::from_second(i64::from(record.timestamp))
            .map(|time| time.strftime("%Y-%m-%dT%H:%M:%S").to_string())
            .unwrap_or_default();
        let payload: Vec<String> = record
            .payload
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        table.push_str(&format!(
            "{recorder},{time},{},{},{}\n",
            record.category,
            record.kind,
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
        assert_eq!(lines[0], "recorder,time,category,type,payload");
        assert_eq!(lines[2], "case,2026-11-27T00:41:33,1,2,64 00 00 3d");
    }
}
