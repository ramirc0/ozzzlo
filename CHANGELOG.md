# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Log tab in the terminal interface that shows the log since startup and follows new lines.
- `sleep-data` command that downloads the raw sleep and sensor logs from the case and buds without erasing them.

## [0.1.0] - 2026-11-27

### Added

- Control of Ozlo Sleepbuds 2 through the charging case over Bluetooth LE.
- `status`, `battery` and `sounds` commands that read the case's state.
- `play`, `pause`, `stop` and `volume` commands for the sounds stored on the buds.
- `timer`, `auto-start` and `bluetooth` commands for the case's settings.
- `alarm` command that lists, adds, edits, toggles and deletes alarms, and sets the case clock before each change.
- `pair` command that bonds the computer with a case in pairing mode.
- Terminal interface with Sounds, Alarms, Settings and Device tabs, live updates and automatic reconnection.
- `dump` and `raw` commands for inspecting the protocol.
- Log file with every frame exchanged, and backtraces on crashes.
- Refusal to send firmware-update, factory-reset and manufacturing commands.
