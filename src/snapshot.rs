//! Everything currently known about the case.

use std::fmt;

use crate::osp::audio::{self, NowPlaying, PlayState, Playback};
use crate::osp::product_info::{self, FirmwareVersion};
use crate::osp::settings::{self, Alarm, AutoStart, BluetoothMode, CaseClock, Sound};
use crate::osp::status::{self, BatteryLevel, BudsConnection, BudsLocation};
use crate::osp::{Frame, Malformed, Operator};

const ALARM_SLOTS: u8 = 30;

/// The case's state, filled in as frames arrive. Unknown values are `None`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub battery: Option<BatteryLevel>,
    pub charging: Option<bool>,
    pub lid_open: Option<bool>,
    pub buds_location: Option<BudsLocation>,
    pub buds_connection: Option<BudsConnection>,
    pub playback: Option<Playback>,
    pub timer_seconds_left: Option<u16>,
    pub volume: Option<u8>,
    pub sleep_timer_minutes: Option<u16>,
    pub auto_start: Option<AutoStart>,
    pub bluetooth_mode: Option<BluetoothMode>,
    pub clock: Option<CaseClock>,
    pub sound_loops: Vec<Sound>,
    pub alarm_sounds: Vec<Sound>,
    pub alarms: Vec<Alarm>,
    pub product_name: Option<String>,
    pub firmware: Option<FirmwareVersion>,
    pub serial_number: Option<String>,
    pub hardware_revision: Option<String>,
    pub osp_version: Option<String>,
}

impl Snapshot {
    /// Folds a frame from the case into the snapshot.
    ///
    /// Only `Status` frames carry state. A payload that does not fit its
    /// known layout is logged and leaves the snapshot unchanged.
    pub fn apply(&mut self, frame: &Frame) {
        if frame.operator != Operator::Status {
            return;
        }
        if let Err(malformed) = self.absorb(frame) {
            log::warn!("cannot decode {malformed}");
        }
    }

    fn absorb(&mut self, frame: &Frame) -> Result<(), Malformed> {
        match frame.function {
            status::BATTERY_LEVEL => self.battery = Some(BatteryLevel::parse(frame)?),
            status::CHARGING_STATE => self.charging = Some(status::flag(frame)?),
            status::CASE_LID_STATE => self.lid_open = Some(status::flag(frame)?),
            status::BUDS_LOCATION => self.buds_location = Some(BudsLocation::parse(frame)?),
            status::BUDS_CONNECTION_STATE => {
                self.buds_connection = Some(BudsConnection::parse(frame)?);
            }
            audio::PLAYING_LOOP => self.playback = Some(Playback::parse(frame)?),
            audio::NOW_PLAYING => {
                let now = NowPlaying::parse(frame)?;
                self.playback = Some(now.playback);
                self.timer_seconds_left = Some(now.timer_seconds_left);
            }
            audio::SOUND_LOOP_VOLUME => self.volume = Some(audio::volume(frame)?),
            settings::SLEEP_TIMER => {
                self.sleep_timer_minutes = Some(settings::sleep_timer_minutes(frame)?);
            }
            settings::AUTO_START => self.auto_start = Some(AutoStart::parse(frame)?),
            settings::BLUETOOTH_MODE => self.bluetooth_mode = Some(BluetoothMode::parse(frame)?),
            settings::CURRENT_DATE_TIME => self.clock = Some(CaseClock::parse(frame)?),
            settings::SOUND_LOOP_LIST => self.sound_loops = Sound::parse_loops(frame)?,
            settings::ALARM_SOUND_LIST => self.alarm_sounds = Sound::parse_alarm_sounds(frame)?,
            settings::ALARMS => self.alarms = Alarm::parse_all(frame)?,
            settings::PRODUCT_NAME => self.product_name = Some(settings::product_name(frame)?),
            product_info::FIRMWARE_VERSION => self.firmware = Some(FirmwareVersion::parse(frame)?),
            product_info::SERIAL_NUMBER => self.serial_number = Some(product_info::text(frame)?),
            product_info::HARDWARE_REVISION => {
                self.hardware_revision = Some(product_info::text(frame)?);
            }
            product_info::OSP_VERSION => self.osp_version = Some(product_info::text(frame)?),
            _ => {}
        }
        Ok(())
    }

    /// The requests that fill a fresh snapshot.
    pub fn survey() -> [Frame; 5] {
        [
            Frame::start(product_info::GET_ALL),
            Frame::start(settings::GET_ALL),
            Frame::start(status::GET_ALL),
            Frame::get(audio::NOW_PLAYING),
            Frame::get(audio::SOUND_LOOP_VOLUME),
        ]
    }

    /// The requests that make `loop_id` the playing sound, in sending order.
    ///
    /// Each one is sent after the case answers the one before.
    pub fn requests_to_play(&self, loop_id: u8) -> Vec<Frame> {
        let mut requests = Vec::new();
        if let Some(current) = self.playback.filter(|current| current.loop_id != loop_id) {
            if current.state == PlayState::Playing {
                requests.push(audio::pause(current.loop_id));
            }
            requests.push(settings::select_loop(loop_id));
        }
        requests.push(audio::play(loop_id));
        requests
    }

    /// The lowest alarm slot not in use.
    pub fn free_alarm_slot(&self) -> Option<u8> {
        (0..ALARM_SLOTS).find(|slot| self.alarms.iter().all(|alarm| alarm.index != *slot))
    }

    /// A display name for a sound loop: its marketing name, else the case's label.
    pub fn loop_name(&self, id: u8) -> String {
        name_of(id, settings::loop_title(id), &self.sound_loops)
    }

    /// A display name for an alarm sound: its marketing name, else the case's label.
    pub fn alarm_sound_name(&self, id: u8) -> String {
        name_of(id, settings::alarm_sound_title(id), &self.alarm_sounds)
    }
}

fn name_of(id: u8, title: Option<&str>, sounds: &[Sound]) -> String {
    let label = sounds.iter().find(|sound| sound.id == id);
    match (title, label) {
        (Some(title), _) => title.to_string(),
        (None, Some(sound)) => sound.label.clone(),
        (None, None) => format!("sound {id}"),
    }
}

fn or_unknown<T: fmt::Display>(value: Option<T>) -> String {
    value.map_or_else(|| "unknown".to_string(), |value| value.to_string())
}

impl fmt::Display for Snapshot {
    /// Writes the multi-line report `ozzzlo status` prints.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let percent =
            |level: Option<u8>| level.map_or("?".to_string(), |level| format!("{level}%"));
        let battery = self.battery;
        let lid = match self.lid_open {
            Some(true) => "lid open",
            Some(false) => "lid closed",
            None => "lid unknown",
        };
        let charging = if self.charging == Some(true) {
            ", charging"
        } else {
            ""
        };
        writeln!(
            f,
            "Case        {}, {lid}{charging}",
            percent(battery.map(|battery| battery.case))
        )?;
        let location = match self.buds_location {
            Some(BudsLocation::InCase) => "in case",
            Some(BudsLocation::OutOfCase) => "out of case",
            Some(BudsLocation::LeftOut) => "left out of case",
            Some(BudsLocation::RightOut) => "right out of case",
            None => "location unknown",
        };
        let signal =
            |rssi: Option<i16>| rssi.map_or("no link".to_string(), |rssi| format!("{rssi} dBm"));
        let connection = self.buds_connection;
        writeln!(
            f,
            "Buds        left {} ({}), right {} ({}), {location}",
            percent(battery.map(|battery| battery.left)),
            signal(connection.and_then(|connection| connection.left)),
            percent(battery.map(|battery| battery.right)),
            signal(connection.and_then(|connection| connection.right)),
        )?;
        let playing = match self.playback {
            Some(playback) => {
                let state = match playback.state {
                    PlayState::Playing => "playing",
                    PlayState::Paused => "paused",
                    PlayState::Stopped => "stopped",
                };
                format!("{}, {state}", self.loop_name(playback.loop_id))
            }
            None => "unknown".to_string(),
        };
        writeln!(
            f,
            "Sound       {playing}, volume {}",
            or_unknown(self.volume)
        )?;
        let timer = match (self.sleep_timer_minutes, self.timer_seconds_left) {
            (Some(0), _) => "off".to_string(),
            (Some(minutes), Some(left)) if left > 0 => {
                format!("{minutes} min, {} min left", left.div_ceil(60))
            }
            (Some(minutes), _) => format!("{minutes} min"),
            (None, _) => "unknown".to_string(),
        };
        writeln!(f, "Sleep timer {timer}")?;
        writeln!(f, "Auto-start  {}", or_unknown(self.auto_start))?;
        writeln!(f, "Bluetooth   {}", or_unknown(self.bluetooth_mode))?;
        let enabled = self.alarms.iter().filter(|alarm| alarm.enabled).count();
        writeln!(f, "Alarms      {enabled} on, {} set", self.alarms.len())?;
        writeln!(f, "Case clock  {}", or_unknown(self.clock))?;
        let firmware = self.firmware.as_ref().map(|firmware| {
            format!(
                "case {} and {}, left {}, right {}",
                firmware.case_bluetooth, firmware.case_radio, firmware.left, firmware.right
            )
        });
        writeln!(f, "Firmware    {}", or_unknown(firmware))?;
        write!(
            f,
            "Serial      {}",
            or_unknown(self.serial_number.as_deref())
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SESSION: &str = "\
        02 02 03 06 64 5b 5b 00 00 00
        02 05 03 01 01
        02 08 03 01 01
        02 09 03 06 01 ff f0 01 ff e4
        02 0f 03 01 01
        05 06 03 0c 07 00 00 08 15 7b 09 01 0a 12 0b 00
        05 09 03 02 01 28
        01 0d 03 02 00 00
        01 10 03 01 00
        01 17 03 03 01 00 00
        01 16 03 07 1a 0b 1b 05 02 23 0e
        01 07 03 0c 02 00 02 4d 30 01 41 03 4d 36 35 01
        01 06 03 08 01 00 06 2d 01 64 3e 08
        01 0b 04 01 05
        00 07 03 06 44 54 48 30 30 31";

    fn replayed() -> Snapshot {
        let mut snapshot = Snapshot::default();
        for line in SESSION.lines() {
            snapshot.apply(&line.parse().unwrap());
        }
        snapshot
    }

    #[test]
    fn a_replayed_session_fills_the_snapshot() {
        let snapshot = replayed();
        assert_eq!(snapshot.battery.unwrap().left, 91);
        assert_eq!(snapshot.lid_open, Some(true));
        assert_eq!(snapshot.buds_location, Some(BudsLocation::OutOfCase));
        assert_eq!(snapshot.playback.unwrap().loop_id, 18);
        assert_eq!(snapshot.volume, Some(40));
        assert_eq!(snapshot.sleep_timer_minutes, Some(0));
        assert_eq!(snapshot.bluetooth_mode, Some(BluetoothMode::Enabled));
        assert_eq!(snapshot.alarms.len(), 1);
        assert_eq!(snapshot.serial_number.as_deref(), Some("DTH001"));
    }

    #[test]
    fn a_malformed_frame_leaves_the_snapshot_unchanged() {
        let mut snapshot = replayed();
        let before = snapshot.clone();
        snapshot.apply(&"02 02 03 01 64".parse().unwrap());
        assert_eq!(snapshot, before);
    }

    #[test]
    fn switching_sounds_pauses_then_selects_then_plays() {
        let hex = |requests: Vec<Frame>| -> Vec<String> {
            requests.iter().map(Frame::to_string).collect()
        };
        let snapshot = replayed();
        assert_eq!(hex(snapshot.requests_to_play(18)), ["05 07 02 03 12 01 00"]);
        assert_eq!(
            hex(snapshot.requests_to_play(6)),
            [
                "05 07 02 03 12 02 00",
                "01 08 02 01 06",
                "05 07 02 03 06 01 00"
            ]
        );
    }

    #[test]
    fn the_free_alarm_slot_skips_used_indexes() {
        assert_eq!(replayed().free_alarm_slot(), Some(1));
    }

    #[test]
    fn sounds_are_named_by_title_then_label_then_id() {
        let snapshot = replayed();
        assert_eq!(snapshot.loop_name(18), "Intimate Rain");
        assert_eq!(snapshot.loop_name(65), "M65");
        assert_eq!(snapshot.loop_name(99), "sound 99");
    }

    #[test]
    fn the_report_names_the_playing_sound() {
        let report = replayed().to_string();
        assert!(report.contains("Sound       Intimate Rain, playing, volume 40"));
        assert!(
            report.contains("Buds        left 91% (-16 dBm), right 91% (-28 dBm), out of case")
        );
        assert!(report.contains("Case clock  2026-11-27 02:35:14"));
    }
}
