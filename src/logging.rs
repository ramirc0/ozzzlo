//! Log file, optional stderr echo, and the panic hook.

use std::backtrace::Backtrace;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Context;
use log::{Level, LevelFilter, Log, Metadata, Record};

const ROTATE_ABOVE_BYTES: u64 = 1024 * 1024;

struct Logger {
    file: Mutex<File>,
    own_level: LevelFilter,
    foreign_level: LevelFilter,
    echo_level: LevelFilter,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let limit = if metadata.target().starts_with(env!("CARGO_CRATE_NAME")) {
            self.own_level
        } else {
            self.foreign_level
        };
        metadata.level() <= limit
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let mut line = format!(
            "{} {:5} {}",
            jiff::ZonedDateTime::now().strftime("%Y-%m-%dT%H:%M:%S%.3f"),
            record.level(),
            record.args()
        );
        if record.level() <= Level::Warn {
            line.push_str(&format!(
                " ({}:{})",
                record.file().unwrap_or("?"),
                record.line().unwrap_or(0)
            ));
        }
        if let Ok(mut file) = self.file.lock() {
            let _ = writeln!(file, "{line}");
        }
        if record.level() <= self.echo_level {
            eprintln!("{line}");
        }
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }
}

/// Where the log file lives: `$XDG_STATE_HOME/ozzzlo/ozzzlo.log`.
pub fn file_path() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
        });
    state.join("ozzzlo").join("ozzzlo.log")
}

/// Starts logging to the file and installs the panic hook.
///
/// The file records debug and above for this crate, and warnings from other
/// crates. `OZZZLO_LOG=trace` records everything from every crate.
/// `verbosity` echoes to stderr: 1 for info, 2 or more for debug.
///
/// # Errors
///
/// Fails if the log directory or file cannot be created, or if a logger is
/// already installed.
pub fn start(verbosity: u8) -> anyhow::Result<()> {
    let path = file_path();
    let directory = path.parent().context("log path has no parent")?;
    fs::create_dir_all(directory).with_context(|| format!("creating {}", directory.display()))?;
    if fs::metadata(&path).is_ok_and(|meta| meta.len() > ROTATE_ABOVE_BYTES) {
        fs::rename(&path, path.with_extension("log.old"))
            .with_context(|| format!("rotating {}", path.display()))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;

    let everything = std::env::var("OZZZLO_LOG").is_ok_and(|level| level == "trace");
    let logger = Logger {
        file: Mutex::new(file),
        own_level: if everything {
            LevelFilter::Trace
        } else {
            LevelFilter::Debug
        },
        foreign_level: if everything {
            LevelFilter::Trace
        } else {
            LevelFilter::Warn
        },
        echo_level: match verbosity {
            0 => LevelFilter::Off,
            1 => LevelFilter::Info,
            _ => LevelFilter::Debug,
        },
    };
    log::set_boxed_logger(Box::new(logger)).context("installing the logger")?;
    log::set_max_level(LevelFilter::Trace);

    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        log::error!("{panic}\n{}", Backtrace::force_capture());
        previous_hook(panic);
        eprintln!("ozzzlo crashed. Details are in {}", file_path().display());
    }));
    Ok(())
}
