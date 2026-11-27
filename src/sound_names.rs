//! Names the user gives to sounds they stored on the buds.
//!
//! The buds keep no names, so these live in a local file with one sound per
//! line: kind, ID and name, such as `alarm 50 Morning Bells`.

use std::fmt;
use std::path::PathBuf;

use crate::osp::sounds::Kind;

/// The user's own names for sounds, by store and ID.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SoundNames {
    names: Vec<(Kind, u8, String)>,
}

impl SoundNames {
    /// Where the names are kept: `$XDG_CONFIG_HOME/ozzzlo/sound-names`.
    pub fn file_path() -> PathBuf {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            });
        config.join("ozzzlo").join("sound-names")
    }

    /// Reads the names file. A missing file holds no names.
    pub fn load() -> Self {
        match std::fs::read_to_string(Self::file_path()) {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::default(),
        }
    }

    /// Reads names from `text`. A line that does not fit is logged and skipped.
    pub fn parse(text: &str) -> Self {
        let mut names = Self::default();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let mut parts = line.splitn(3, ' ');
            let kind = match parts.next() {
                Some("masking") => Some(Kind::Masking),
                Some("alarm") => Some(Kind::Alarm),
                _ => None,
            };
            let id = parts.next().and_then(|id| id.parse().ok());
            match (kind, id, parts.next()) {
                (Some(kind), Some(id), Some(name)) => names.name(kind, id, name),
                _ => log::warn!("skipping sound name line `{line}`"),
            }
        }
        names
    }

    /// Writes the names file, creating its directory.
    ///
    /// # Errors
    ///
    /// Returns the error from creating the directory or writing the file.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::file_path();
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        std::fs::write(path, self.to_string())
    }

    /// The user's name for sound `id` of `kind`.
    pub fn name_of(&self, kind: Kind, id: u8) -> Option<&str> {
        let named = self.names.iter().find(|(k, i, _)| (*k, *i) == (kind, id));
        named.map(|(_, _, name)| name.as_str())
    }

    /// Names sound `id` of `kind`, replacing any name it had.
    pub fn name(&mut self, kind: Kind, id: u8, name: &str) {
        self.forget(kind, id);
        self.names.push((kind, id, name.trim().to_string()));
    }

    /// Drops the name of sound `id` of `kind`.
    pub fn forget(&mut self, kind: Kind, id: u8) {
        self.names.retain(|(k, i, _)| (*k, *i) != (kind, id));
    }
}

impl fmt::Display for SoundNames {
    /// Writes the file form that [`SoundNames::parse`] reads.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (kind, id, name) in &self.names {
            let kind = match kind {
                Kind::Masking => "masking",
                Kind::Alarm => "alarm",
            };
            writeln!(f, "{kind} {id} {name}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_survive_a_round_trip_through_the_file_form() {
        let mut names = SoundNames::default();
        names.name(Kind::Alarm, 50, "Great Fairy's Fountain");
        names.name(Kind::Masking, 50, "Night Train");
        let reread = SoundNames::parse(&names.to_string());
        assert_eq!(reread, names);
        assert_eq!(
            reread.name_of(Kind::Alarm, 50),
            Some("Great Fairy's Fountain")
        );
        assert_eq!(reread.name_of(Kind::Masking, 50), Some("Night Train"));
        assert_eq!(reread.name_of(Kind::Alarm, 8), None);
    }

    #[test]
    fn renaming_replaces_and_forgetting_removes() {
        let mut names = SoundNames::default();
        names.name(Kind::Alarm, 50, "First");
        names.name(Kind::Alarm, 50, "Second");
        assert_eq!(names.to_string(), "alarm 50 Second\n");
        names.forget(Kind::Alarm, 50);
        assert_eq!(names.name_of(Kind::Alarm, 50), None);
    }

    #[test]
    fn lines_that_do_not_fit_are_skipped() {
        let names = SoundNames::parse("nonsense\nalarm x Bad\n\nalarm 9 Good One\n");
        assert_eq!(names.to_string(), "alarm 9 Good One\n");
    }
}
