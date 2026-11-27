# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed

- `pair` no longer tells the user to hold the case button when the case is already in pairing mode or has not been reached yet. It reports each situation as it finds it.
- `pair` no longer fails with "Not connected" when the link drops as the case enters pairing mode. It keeps retrying for two minutes.
- The terminal interface no longer reconnects two seconds after the case announces it is going idle, which kept waking the case in a loop. It waits 30 seconds.
- A command no longer waits 20 seconds when the phone app holds the case. It gives up after 8 seconds, says the phone app may be connected, and cancels the pending connection.
- The terminal interface misread the case's disconnect reason, showing "reason 172" where the case had gone idle.
- Connecting no longer hangs for two minutes when BlueZ reports the LE link up but has lost the case's services.

### Added

- Log tab in the terminal interface that shows the log since startup and follows new lines, with line and page scrolling.
- `sleep-data` command that downloads the sleep and sensor logs from the case and buds without erasing them, and splits them into CSV records. `--offline` rebuilds the CSV from saved logs.

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
