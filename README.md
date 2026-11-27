# ozzzlo

Control Ozlo Sleepbuds 2 from a Linux desktop, with a command line and a terminal interface.

ozzzlo is an independent project. It is not affiliated with or endorsed by Ozlo or Drowsy Digital, Inc.

## Description

Ozlo ships phone apps only. ozzzlo talks to the Sleepbuds charging case over Bluetooth LE, using the same control protocol as those apps.
The audio link stays free, so the laptop can stream to the buds while ozzzlo controls them.

It can:

* show battery, bud location, lid state, firmware versions and the case clock
* play, pause and stop the sounds stored on the buds, and switch between them
* change the volume, the sleep timer, auto-start and the Bluetooth mode
* list, add, edit, toggle and delete alarms
* run as a terminal interface that follows the case live

It cannot:

* upload new sounds to the buds
* interpret sleep and health data. `ozzzlo sleep-data` saves the logs as timestamped records with raw payloads
* update firmware
* talk to the case over USB
* find a case the computer has never paired with
* run on macOS or Windows

The protocol was worked out by observation.
It was tested on one Sleepbuds 2 case with firmware 02.11.2606025. Other firmware and the first-generation Sleepbuds are untested.

## Getting Started

### Dependencies

* Linux with BlueZ. Version 5.88 was tested. Connecting without touching the audio link needs a BlueZ that exposes `org.bluez.Bearer.LE1`.
* A Rust toolchain with edition 2024 support, and a C compiler. The build compiles libdbus from source.
* A terminal of at least 80 by 24 for the interface.

### Installing

```
git clone <this repository>
cd ozzzlo
cargo install --path .
```

### Executing program

* Pair the case twice: once for audio in the desktop's Bluetooth settings, and once for control with ozzzlo. [docs/pairing.md](docs/pairing.md) walks through it and lists what each message means.
```
ozzzlo pair
```
* Mark the case as trusted in the desktop's Bluetooth settings, so the computer reconnects audio by itself. The case drops a host that holds only the control link after a few seconds, and an audio link prevents that. In the interface, `A` connects audio by hand.
* Open the interface, or run a single command.
```
ozzzlo
ozzzlo status
ozzzlo play rain
ozzzlo volume 35
ozzzlo timer 45
ozzzlo alarm add 07:30 --days weekdays
```

## Help

```
ozzzlo --help
ozzzlo alarm --help
```

* The case drops a host that holds only the control link after a few seconds of idleness. Connect the computer to the buds for audio as well, and the control link stays up.
* One ozzzlo can hold the control link at a time. Close the interface before running a command.
* The case serves one control host at a time. While the phone app is open and connected, ozzzlo cannot reach the case and gives up after 8 seconds. While ozzzlo is connected, the phone app cannot control the buds.
* ozzzlo keeps a log at `~/.local/state/ozzzlo/ozzzlo.log`. It records every frame exchanged with the case, plus the case's Bluetooth address and serial number. `-v` echoes it to the terminal.
* After a firmware update, `ozzzlo dump` prints every readable function as raw hex. A diff of two dumps shows what the update changed. Decode failures appear in the log as `WARN` lines that name the function.
* `RUST_BACKTRACE=1` adds a stack trace to errors in the log. A crash always logs one.
* ozzzlo refuses to send firmware-update, factory-reset and manufacturing commands.

## Authors

Christian Ramirez

## Version History

See [CHANGELOG.md](CHANGELOG.md).

## License

This project is licensed under the MIT License - see the LICENSE file for details

## Acknowledgments

* [bluer](https://github.com/bluez/bluer), the Rust interface to BlueZ
* [ratatui](https://github.com/ratatui/ratatui), the terminal interface library
