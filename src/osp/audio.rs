//! The AudioManagement block: playback and volume.

use super::{Frame, Function, Malformed};

pub const NOW_PLAYING: Function = Function { block: 5, id: 6 };
pub const PLAYING_LOOP: Function = Function { block: 5, id: 7 };
pub const SOUND_LOOP_VOLUME: Function = Function { block: 5, id: 9 };

/// Whether a sound loop is audible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayState {
    Stopped,
    Playing,
    Paused,
}

impl PlayState {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Stopped),
            1 => Some(Self::Playing),
            2 => Some(Self::Paused),
            _ => None,
        }
    }
}

/// The selected sound loop and what it is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Playback {
    pub loop_id: u8,
    pub state: PlayState,
}

impl Playback {
    /// Reads a `PlayingLoop` reply: loop ID, state, preview flag.
    pub fn parse(frame: &Frame) -> Result<Self, Malformed> {
        let expected = "loop ID, play state from 0 to 2, preview flag";
        let [loop_id, state, _preview] = frame.payload[..] else {
            return Err(frame.malformed(expected));
        };
        let state = PlayState::from_byte(state).ok_or_else(|| frame.malformed(expected))?;
        Ok(Self { loop_id, state })
    }
}

/// The unprompted playback report the case pushes when playback changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NowPlaying {
    pub playback: Playback,
    /// Seconds until the sleep timer stops playback. Zero when no timer runs.
    pub timer_seconds_left: u16,
}

impl NowPlaying {
    /// Reads a `NowPlaying` reply: tag-value pairs.
    pub fn parse(frame: &Frame) -> Result<Self, Malformed> {
        let expected = "tags 7 and 8 with 16-bit values, tags 9 to 11 with bytes";
        let (mut timer, mut state, mut loop_id) = (None, None, None);
        let mut rest = &frame.payload[..];
        while !rest.is_empty() {
            rest = match *rest {
                [7, high, low, ref tail @ ..] => {
                    timer = Some(u16::from_be_bytes([high, low]));
                    tail
                }
                [8, _, _, ref tail @ ..] | [11, _, ref tail @ ..] => tail,
                [9, value, ref tail @ ..] => {
                    state = PlayState::from_byte(value);
                    tail
                }
                [10, value, ref tail @ ..] => {
                    loop_id = Some(value);
                    tail
                }
                _ => return Err(frame.malformed(expected)),
            };
        }
        match (timer, state, loop_id) {
            (Some(timer_seconds_left), Some(state), Some(loop_id)) => Ok(Self {
                playback: Playback { loop_id, state },
                timer_seconds_left,
            }),
            _ => Err(frame.malformed(expected)),
        }
    }
}

/// Reads a `SoundLoopVolume` reply. The level is the second byte.
pub fn volume(frame: &Frame) -> Result<u8, Malformed> {
    match frame.payload[..] {
        [_, level] => Ok(level),
        _ => Err(frame.malformed("two bytes, the second being the level")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_reads_loop_and_state() {
        let frame: Frame = "05 07 03 03 12 01 00".parse().unwrap();
        let expected = Playback {
            loop_id: 18,
            state: PlayState::Playing,
        };
        assert_eq!(Playback::parse(&frame), Ok(expected));
    }

    #[test]
    fn now_playing_reads_tagged_values() {
        let frame: Frame = "05 06 03 0c 07 00 00 08 15 7b 09 01 0a 12 0b 00"
            .parse()
            .unwrap();
        let now = NowPlaying::parse(&frame).unwrap();
        assert_eq!(now.playback.loop_id, 18);
        assert_eq!(now.playback.state, PlayState::Playing);
        assert_eq!(now.timer_seconds_left, 0);
    }

    #[test]
    fn now_playing_rejects_an_unknown_tag() {
        let frame: Frame = "05 06 03 02 63 00".parse().unwrap();
        assert!(NowPlaying::parse(&frame).is_err());
    }

    #[test]
    fn volume_is_the_second_byte() {
        let frame: Frame = "05 09 03 02 01 28".parse().unwrap();
        assert_eq!(volume(&frame), Ok(40));
    }
}
