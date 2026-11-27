//! The terminal interface: a fixed header over numbered tabs.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListState, Paragraph};
use ratatui::{DefaultTerminal, Frame as Canvas};
use tokio::sync::mpsc;
use tokio::time::{Instant, interval_at, sleep_until};

use crate::case::{self, Case};
use crate::logging;
use crate::osp::audio::{self, PlayState};
use crate::osp::settings::{self, Alarm, AutoStart, BluetoothMode, CaseClock, Days};
use crate::osp::status::{self, BudsLocation};
use crate::osp::{Frame, Function, Operator};
use crate::snapshot::Snapshot;

const MIN_WIDTH: u16 = 80;
const MIN_HEIGHT: u16 = 24;
const VOLUME_STEP: u8 = 5;
const LOG_PAGE_LINES: isize = 10;
const TIMER_STEP_MINUTES: u16 = 15;
const TIMER_MAX_MINUTES: u16 = 480;
const FIRST_RETRY: Duration = Duration::from_secs(2);
const LONGEST_RETRY: Duration = Duration::from_secs(30);
const BATTERY_PERIOD: Duration = Duration::from_secs(300);
const DISCONNECT: Function = Function { block: 4, id: 2 };
// The case drops a control-only host after a few seconds. An audio link keeps it awake.
const IDLE_NOTICE: &str = "the case went idle. It stays up only with an audio link. A connects one";
const AUTO_START_CHOICES: [AutoStart; 8] = [
    AutoStart::Immediately,
    AutoStart::Off,
    AutoStart::WhenAsleep,
    AutoStart::AfterMinutes(15),
    AutoStart::AfterMinutes(30),
    AutoStart::AfterMinutes(45),
    AutoStart::AfterMinutes(60),
    AutoStart::AfterMinutes(120),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    Sounds,
    Alarms,
    Settings,
    Device,
    Log,
}

const TABS: [(Tab, &str, &str); 5] = [
    (
        Tab::Sounds,
        "Sounds",
        "j/k move · enter play · space pause · +/- vol · 1-5 tab · ? help · q quit",
    ),
    (
        Tab::Alarms,
        "Alarms",
        "j/k move · a add · e edit · d delete · enter on/off · ? help · q quit",
    ),
    (
        Tab::Settings,
        "Settings",
        "j/k move · h/l change · space pause · +/- vol · ? help · q quit",
    ),
    (
        Tab::Device,
        "Device",
        "r refresh · c connect · A audio · 1-5 tab · ? help · q quit",
    ),
    (
        Tab::Log,
        "Log",
        "j/k line · u/d page · g top · G follow · 1-5 tab · ? help · q quit",
    ),
];

const HELP: [&str; 14] = [
    "1 to 5      switch tab (tab and shift-tab also work)",
    "j k         move down and up (arrow keys also work)",
    "space       play or pause the current sound",
    "+ -         volume up and down",
    "enter       play the selected sound, or toggle the selected alarm",
    "a e d       add, edit or delete an alarm",
    "h l         change the selected setting",
    "r           ask the case for everything again",
    "c           connect now",
    "A           connect this computer to the buds for audio",
    "q           quit",
    "",
    "In the alarm dialog: tab moves between fields, h and l change",
    "a value, space toggles a day, enter saves, esc cancels.",
];

/// Whether the case is reachable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    Connecting,
    Up,
    /// Not connected, with the reason.
    Down(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Hour,
    Minute,
    Days,
    Sound,
    Volume,
    Enabled,
}

const FIELDS: [Field; 6] = [
    Field::Hour,
    Field::Minute,
    Field::Days,
    Field::Sound,
    Field::Volume,
    Field::Enabled,
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct AlarmDialog {
    alarm: Alarm,
    is_new: bool,
    field: usize,
    day: u8,
}

/// What the interface is showing, apart from the case's own state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    tab: Tab,
    sound_row: usize,
    alarm_row: usize,
    setting_row: usize,
    /// Lines scrolled up from the newest log line. Zero follows the log.
    log_scroll: usize,
    dialog: Option<AlarmDialog>,
    deleting: Option<Alarm>,
    showing_help: bool,
    /// Shown in the title bar.
    pub link: Link,
    /// The last thing the case refused or reported, shown above the key hints.
    pub notice: Option<String>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            tab: Tab::Sounds,
            sound_row: 0,
            alarm_row: 0,
            setting_row: 0,
            log_scroll: 0,
            dialog: None,
            deleting: None,
            showing_help: false,
            link: Link::Connecting,
            notice: None,
        }
    }
}

/// What a key press asks the event loop to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Stay,
    /// Send these requests in order, each after the previous one is answered.
    Send(Vec<Frame>),
    Reconnect,
    /// Connect this computer to the buds for audio, then reconnect control.
    ConnectAudio,
    Quit,
}

fn stepped(row: usize, delta: isize, count: usize) -> usize {
    row.saturating_add_signed(delta)
        .min(count.saturating_sub(1))
}

/// Moves a log view `delta` lines down, where `scroll` counts lines up from
/// the newest of `line_count`.
fn scrolled(scroll: usize, delta: isize, line_count: usize) -> usize {
    scroll.min(line_count).saturating_add_signed(-delta)
}

fn cycled(position: usize, delta: isize, count: usize) -> usize {
    (position + count).saturating_add_signed(delta) % count
}

/// Updates `app` for a key press and says what to do next.
pub fn on_key(app: &mut App, snapshot: &Snapshot, key: KeyEvent) -> Action {
    if key.kind != KeyEventKind::Press {
        return Action::Stay;
    }
    if app.showing_help {
        app.showing_help = false;
        return Action::Stay;
    }
    if let Some(alarm) = app.deleting.take() {
        return match key.code {
            KeyCode::Char('y') => Action::Send(vec![
                settings::delete_alarm(alarm.index),
                Frame::get(settings::ALARMS),
            ]),
            _ => Action::Stay,
        };
    }
    if app.dialog.is_some() {
        return on_dialog_key(app, snapshot, key.code);
    }
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('q') => return Action::Quit,
        KeyCode::Char('c') if control => return Action::Quit,
        KeyCode::Char('c') => return Action::Reconnect,
        KeyCode::Char('A') => return Action::ConnectAudio,
        KeyCode::Char('?') => app.showing_help = true,
        KeyCode::Char('r') => return Action::Send(Snapshot::survey().to_vec()),
        KeyCode::Char('1') => app.tab = Tab::Sounds,
        KeyCode::Char('2') => app.tab = Tab::Alarms,
        KeyCode::Char('3') => app.tab = Tab::Settings,
        KeyCode::Char('4') => app.tab = Tab::Device,
        KeyCode::Char('5') => app.tab = Tab::Log,
        KeyCode::Tab | KeyCode::BackTab => {
            let delta = if key.code == KeyCode::Tab { 1 } else { -1 };
            let position = TABS.iter().position(|(tab, ..)| *tab == app.tab);
            app.tab = TABS[cycled(position.unwrap_or(0), delta, TABS.len())].0;
        }
        KeyCode::Char(' ') => {
            if let Some(playback) = snapshot.playback {
                let request = match playback.state {
                    PlayState::Playing => audio::pause(playback.loop_id),
                    _ => audio::play(playback.loop_id),
                };
                return Action::Send(vec![request]);
            }
        }
        KeyCode::Char('+' | '=' | '-') => {
            if let Some(volume) = snapshot.volume {
                let level = match key.code {
                    KeyCode::Char('-') => volume.saturating_sub(VOLUME_STEP),
                    _ => volume.saturating_add(VOLUME_STEP).min(100),
                };
                return Action::Send(vec![audio::change_volume_to(level)]);
            }
        }
        KeyCode::Char('j') | KeyCode::Down | KeyCode::Char('k') | KeyCode::Up => {
            let delta = match key.code {
                KeyCode::Char('j') | KeyCode::Down => 1,
                _ => -1,
            };
            match app.tab {
                Tab::Sounds => {
                    app.sound_row = stepped(app.sound_row, delta, snapshot.sound_loops.len());
                }
                Tab::Alarms => {
                    app.alarm_row = stepped(app.alarm_row, delta, snapshot.alarms.len());
                }
                Tab::Settings => app.setting_row = stepped(app.setting_row, delta, 3),
                Tab::Device => {}
                Tab::Log => {
                    app.log_scroll = scrolled(app.log_scroll, delta, logging::recent().len());
                }
            }
        }
        code => return on_tab_key(app, snapshot, code),
    }
    Action::Stay
}

fn on_tab_key(app: &mut App, snapshot: &Snapshot, code: KeyCode) -> Action {
    let selected_alarm = snapshot.alarms.get(app.alarm_row).copied();
    match (app.tab, code) {
        (Tab::Sounds, KeyCode::Enter) => {
            if let Some(sound) = snapshot.sound_loops.get(app.sound_row) {
                return Action::Send(snapshot.requests_to_play(sound.id));
            }
        }
        (Tab::Alarms, KeyCode::Char('a')) => {
            let slot = snapshot.free_alarm_slot();
            let sound = snapshot.alarm_sounds.first();
            match (slot, sound) {
                (Some(index), Some(sound)) => {
                    let alarm = Alarm {
                        index,
                        hour: 7,
                        minute: 0,
                        enabled: true,
                        volume: 60,
                        days: Days(0x7f),
                        sound: sound.id,
                    };
                    app.dialog = Some(AlarmDialog {
                        alarm,
                        is_new: true,
                        field: 0,
                        day: 1,
                    });
                }
                _ => app.notice = Some("no free alarm slot or no alarm sound".to_string()),
            }
        }
        (Tab::Alarms, KeyCode::Char('e')) => {
            app.dialog = selected_alarm.map(|alarm| AlarmDialog {
                alarm,
                is_new: false,
                field: 0,
                day: 1,
            });
        }
        (Tab::Alarms, KeyCode::Char('d')) => app.deleting = selected_alarm,
        (Tab::Alarms, KeyCode::Enter) => {
            if let Some(alarm) = selected_alarm {
                let toggled = Alarm {
                    enabled: !alarm.enabled,
                    ..alarm
                };
                return Action::Send(requests_to_store(&toggled));
            }
        }
        (Tab::Log, KeyCode::PageUp | KeyCode::Char('u')) => {
            app.log_scroll = scrolled(app.log_scroll, -LOG_PAGE_LINES, logging::recent().len());
        }
        (Tab::Log, KeyCode::PageDown | KeyCode::Char('d')) => {
            app.log_scroll = scrolled(app.log_scroll, LOG_PAGE_LINES, logging::recent().len());
        }
        (Tab::Log, KeyCode::Char('g')) => app.log_scroll = usize::MAX,
        (Tab::Log, KeyCode::Char('G')) => app.log_scroll = 0,
        (Tab::Settings, KeyCode::Char('h') | KeyCode::Left) => {
            return changed_setting(app.setting_row, -1, snapshot);
        }
        (Tab::Settings, KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter) => {
            return changed_setting(app.setting_row, 1, snapshot);
        }
        _ => {}
    }
    Action::Stay
}

/// Alarms fire by the case's clock, so each alarm write sets the clock first.
fn requests_to_store(alarm: &Alarm) -> Vec<Frame> {
    vec![
        settings::set_clock(&CaseClock::now()),
        settings::store_alarm(alarm),
    ]
}

fn changed_setting(row: usize, delta: isize, snapshot: &Snapshot) -> Action {
    let request = match row {
        0 => {
            // `disabled` cuts streaming and is left to the command line.
            let mode = match snapshot.bluetooth_mode {
                Some(BluetoothMode::Enabled) => BluetoothMode::PowerSaving,
                _ => BluetoothMode::Enabled,
            };
            return Action::Send(vec![
                settings::bluetooth(mode),
                Frame::get(settings::BLUETOOTH_MODE),
            ]);
        }
        1 => {
            let current = AUTO_START_CHOICES
                .iter()
                .position(|choice| Some(*choice) == snapshot.auto_start);
            let next = cycled(current.unwrap_or(0), delta, AUTO_START_CHOICES.len());
            settings::auto_start(AUTO_START_CHOICES[next])
        }
        _ => {
            let minutes = snapshot.sleep_timer_minutes.unwrap_or(0);
            let minutes = if delta > 0 {
                (minutes + TIMER_STEP_MINUTES).min(TIMER_MAX_MINUTES)
            } else {
                minutes.saturating_sub(TIMER_STEP_MINUTES)
            };
            settings::sleep_after(minutes)
        }
    };
    Action::Send(vec![request])
}

fn on_dialog_key(app: &mut App, snapshot: &Snapshot, code: KeyCode) -> Action {
    let Some(dialog) = app.dialog.as_mut() else {
        return Action::Stay;
    };
    let field = FIELDS[dialog.field];
    let alarm = &mut dialog.alarm;
    match code {
        KeyCode::Esc => app.dialog = None,
        KeyCode::Enter => {
            let requests = requests_to_store(alarm);
            app.dialog = None;
            return Action::Send(requests);
        }
        KeyCode::Tab | KeyCode::Down | KeyCode::Char('j') => {
            dialog.field = cycled(dialog.field, 1, FIELDS.len());
        }
        KeyCode::BackTab | KeyCode::Up | KeyCode::Char('k') => {
            dialog.field = cycled(dialog.field, -1, FIELDS.len());
        }
        KeyCode::Char(' ') if field == Field::Days => alarm.days.0 ^= 1 << dialog.day,
        KeyCode::Char(' ') if field == Field::Enabled => alarm.enabled = !alarm.enabled,
        KeyCode::Left | KeyCode::Char('h') | KeyCode::Right | KeyCode::Char('l') => {
            let delta: isize = match code {
                KeyCode::Left | KeyCode::Char('h') => -1,
                _ => 1,
            };
            match field {
                Field::Hour => alarm.hour = cycled(alarm.hour.into(), delta, 24) as u8,
                Field::Minute => alarm.minute = cycled(alarm.minute.into(), delta * 5, 60) as u8,
                Field::Days => dialog.day = cycled(dialog.day.into(), delta, 7) as u8,
                Field::Sound => {
                    let sounds = &snapshot.alarm_sounds;
                    let current = sounds.iter().position(|sound| sound.id == alarm.sound);
                    if !sounds.is_empty() {
                        let next = cycled(current.unwrap_or(0), delta, sounds.len());
                        alarm.sound = sounds[next].id;
                    }
                }
                Field::Volume => {
                    alarm.volume = if delta > 0 {
                        alarm.volume.saturating_add(VOLUME_STEP).min(100)
                    } else {
                        alarm.volume.saturating_sub(VOLUME_STEP)
                    };
                }
                Field::Enabled => alarm.enabled = !alarm.enabled,
            }
        }
        _ => {}
    }
    Action::Stay
}

fn bar(level: u8, full: u8, width: usize) -> String {
    let filled = usize::from(level.min(full)) * width / usize::from(full.max(1));
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn selected() -> Style {
    Style::new().add_modifier(Modifier::REVERSED)
}

/// Dimmed text in the terminal's own foreground. A fixed dark gray vanishes
/// on some color schemes.
fn dim() -> Style {
    Style::new().add_modifier(Modifier::DIM)
}

/// Styles hints such as `j/k move · q quit`: keys stand out, actions stay quiet.
fn key_hints(hints: &'static str) -> Line<'static> {
    let key = Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD);
    let mut spans = vec![Span::raw(" ")];
    for (position, hint) in hints.split(" · ").enumerate() {
        if position > 0 {
            spans.push(Span::styled(" · ", dim()));
        }
        let (keys, action) = hint.split_once(' ').unwrap_or((hint, ""));
        spans.push(Span::styled(keys, key));
        spans.push(Span::raw(format!(" {action}")));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// Paints the whole interface.
pub fn draw(canvas: &mut Canvas, app: &App, snapshot: &Snapshot) {
    let area = canvas.area();
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let message = format!("terminal too small, ozzzlo needs {MIN_WIDTH}x{MIN_HEIGHT}");
        canvas.render_widget(Paragraph::new(message), area);
        return;
    }
    let lid = match snapshot.lid_open {
        Some(true) => " · lid open",
        Some(false) => " · lid closed",
        None => "",
    };
    let link = match &app.link {
        Link::Up => Span::styled(format!("● connected{lid}"), Style::new().fg(Color::Green)),
        Link::Connecting => Span::styled("○ connecting", Style::new().fg(Color::Yellow)),
        Link::Down(reason) => Span::styled(format!("○ {reason}"), Style::new().fg(Color::Red)),
    };
    let hints = TABS.iter().find(|(tab, ..)| *tab == app.tab);
    let frame = Block::bordered()
        .title_top(" ozzzlo ")
        .title_top(Line::from(vec![Span::raw(" "), link, Span::raw(" ")]).right_aligned())
        .title_bottom(key_hints(hints.map_or("", |(_, _, hints)| hints)));
    let inside = frame.inner(area);
    canvas.render_widget(frame, area);
    let [header, tabs, body, notice] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(inside);

    let faded = if app.link == Link::Up {
        Style::new()
    } else {
        dim()
    };
    canvas.render_widget(Paragraph::new(header_lines(snapshot)).style(faded), header);
    canvas.render_widget(Paragraph::new(tab_line(app.tab, tabs.width)), tabs);
    match app.tab {
        Tab::Sounds => draw_sounds(canvas, body, app, snapshot),
        Tab::Alarms => draw_alarms(canvas, body, app, snapshot),
        Tab::Settings => draw_settings(canvas, body, app, snapshot),
        Tab::Device => draw_device(canvas, body, app, snapshot),
        Tab::Log => draw_log(canvas, body, app.log_scroll, &logging::recent()),
    }
    if let Some(text) = &app.notice {
        let warning = Style::new().fg(Color::Yellow);
        canvas.render_widget(Paragraph::new(format!(" {text}")).style(warning), notice);
    }

    if let Some(dialog) = &app.dialog {
        draw_dialog(canvas, area, dialog, snapshot);
    }
    if let Some(alarm) = &app.deleting {
        let question = format!(
            "Delete the {:02}:{:02} alarm? y to confirm",
            alarm.hour, alarm.minute
        );
        popup(canvas, area, " Delete alarm ", vec![Line::raw(question)]);
    }
    if app.showing_help {
        popup(
            canvas,
            area,
            " Keys ",
            HELP.iter().map(|line| Line::raw(*line)).collect(),
        );
    }
}

fn header_lines(snapshot: &Snapshot) -> Vec<Line<'static>> {
    let level = |name: &str, percent: Option<u8>| -> Vec<Span<'static>> {
        let Some(percent) = percent else {
            return vec![Span::raw(format!("{name} ?   "))];
        };
        let color = if percent < 20 {
            Color::Red
        } else {
            Color::Reset
        };
        vec![
            Span::raw(format!("{name} ")),
            Span::styled(bar(percent, 100, 8), Style::new().fg(color)),
            Span::raw(format!(" {percent}%   ")),
        ]
    };
    let battery = snapshot.battery;
    let mut first = vec![Span::raw(" ")];
    first.extend(level("Case", battery.map(|battery| battery.case)));
    first.extend(level("Left", battery.map(|battery| battery.left)));
    first.extend(level("Right", battery.map(|battery| battery.right)));
    first.push(Span::raw(match snapshot.buds_location {
        Some(BudsLocation::InCase) => "Buds in case",
        Some(BudsLocation::OutOfCase) => "Buds out",
        Some(BudsLocation::LeftOut) => "Left bud out",
        Some(BudsLocation::RightOut) => "Right bud out",
        None => "",
    }));

    let playing = match snapshot.playback {
        Some(playback) => {
            let symbol = match playback.state {
                PlayState::Playing => "▶",
                PlayState::Paused => "‖",
                PlayState::Stopped => "■",
            };
            format!("{symbol} {}", snapshot.loop_name(playback.loop_id))
        }
        None => "■ nothing known yet".to_string(),
    };
    let volume = match snapshot.volume {
        Some(volume) => format!("Volume {} {volume}", bar(volume, 100, 12)),
        None => "Volume ?".to_string(),
    };
    let timer = match (snapshot.sleep_timer_minutes, snapshot.timer_seconds_left) {
        (Some(0), _) => "Timer off".to_string(),
        (Some(_), Some(left)) if left > 0 => format!("Timer {} min left", left.div_ceil(60)),
        (Some(minutes), _) => format!("Timer {minutes} min"),
        (None, _) => String::new(),
    };
    vec![
        Line::from(first),
        Line::raw(format!(" {playing}   {volume}   {timer}")),
    ]
}

fn tab_line(current: Tab, width: u16) -> Line<'static> {
    let mut spans = vec![Span::styled("─", dim())];
    let mut used = 1;
    for (number, (tab, name, _)) in TABS.iter().enumerate() {
        let (label, style) = if *tab == current {
            (
                format!("[{} {name}]", number + 1),
                Style::new().add_modifier(Modifier::BOLD),
            )
        } else {
            (format!(" {} {name} ", number + 1), Style::new())
        };
        used += label.chars().count() + 1;
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    let rule = usize::from(width).saturating_sub(used);
    spans.push(Span::styled("─".repeat(rule), dim()));
    Line::from(spans)
}

fn draw_rows(canvas: &mut Canvas, area: Rect, rows: Vec<Line<'static>>, row: usize, empty: &str) {
    if rows.is_empty() {
        canvas.render_widget(Paragraph::new(format!("\n   {empty}")).style(dim()), area);
        return;
    }
    let [_, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let list = List::new(rows).highlight_style(selected());
    let mut state = ListState::default().with_selected(Some(row));
    canvas.render_stateful_widget(list, list_area, &mut state);
}

fn draw_sounds(canvas: &mut Canvas, area: Rect, app: &App, snapshot: &Snapshot) {
    let current = snapshot.playback.map(|playback| playback.loop_id);
    let rows = snapshot
        .sound_loops
        .iter()
        .map(|sound| {
            let marker = if current == Some(sound.id) {
                "▶"
            } else {
                " "
            };
            Line::raw(format!(
                "   {marker} {:<24} {}",
                snapshot.loop_name(sound.id),
                sound.label
            ))
        })
        .collect();
    draw_rows(canvas, area, rows, app.sound_row, "No sounds listed yet.");
}

fn draw_alarms(canvas: &mut Canvas, area: Rect, app: &App, snapshot: &Snapshot) {
    let rows = snapshot
        .alarms
        .iter()
        .map(|alarm| {
            Line::raw(format!(
                "   {:02}:{:02}   {:<28} {:<18} vol {:<3}  {}",
                alarm.hour,
                alarm.minute,
                alarm.days.to_string(),
                snapshot.alarm_sound_name(alarm.sound),
                alarm.volume,
                if alarm.enabled { "on" } else { "off" }
            ))
        })
        .collect();
    draw_rows(
        canvas,
        area,
        rows,
        app.alarm_row,
        "No alarms. Press a to add one.",
    );
}

fn draw_settings(canvas: &mut Canvas, area: Rect, app: &App, snapshot: &Snapshot) {
    let shown = |value: Option<String>| value.unwrap_or_else(|| "unknown".to_string());
    let timer = snapshot.sleep_timer_minutes.map(|minutes| match minutes {
        0 => "off".to_string(),
        minutes => format!("{minutes} min"),
    });
    let rows = [
        (
            "Bluetooth",
            shown(snapshot.bluetooth_mode.map(|mode| mode.to_string())),
        ),
        (
            "Auto-start",
            shown(snapshot.auto_start.map(|mode| mode.to_string())),
        ),
        ("Sleep timer", shown(timer)),
    ]
    .into_iter()
    .map(|(name, value)| Line::raw(format!("   {name:<14} ‹ {value} ›")))
    .collect();
    draw_rows(canvas, area, rows, app.setting_row, "");
}

fn draw_device(canvas: &mut Canvas, area: Rect, app: &App, snapshot: &Snapshot) {
    let shown = |value: Option<String>| value.unwrap_or_else(|| "unknown".to_string());
    let firmware = snapshot.firmware.as_ref();
    let link = match &app.link {
        Link::Up => "connected over Bluetooth LE".to_string(),
        Link::Connecting => "connecting".to_string(),
        Link::Down(reason) => reason.clone(),
    };
    let signal = snapshot.buds_connection.map(|buds| {
        let dbm =
            |rssi: Option<i16>| rssi.map_or("no link".to_string(), |rssi| format!("{rssi} dBm"));
        format!("left {}, right {}", dbm(buds.left), dbm(buds.right))
    });
    let facts = [
        ("Name", shown(snapshot.product_name.clone())),
        ("Serial", shown(snapshot.serial_number.clone())),
        ("Hardware", shown(snapshot.hardware_revision.clone())),
        (
            "Case firmware",
            shown(firmware.map(|firmware| {
                format!("{} and {}", firmware.case_bluetooth, firmware.case_radio)
            })),
        ),
        (
            "Left bud",
            shown(firmware.map(|firmware| firmware.left.clone())),
        ),
        (
            "Right bud",
            shown(firmware.map(|firmware| firmware.right.clone())),
        ),
        ("Protocol", shown(snapshot.osp_version.clone())),
        (
            "Case clock",
            shown(snapshot.clock.map(|clock| clock.to_string())),
        ),
        ("Bud signal", shown(signal)),
        ("Link", link),
        ("Log file", logging::file_path().display().to_string()),
    ];
    let mut lines = vec![Line::raw("")];
    lines.extend(
        facts
            .into_iter()
            .map(|(name, value)| Line::raw(format!("   {name:<14} {value}"))),
    );
    canvas.render_widget(Paragraph::new(lines), area);
}

/// Shows the newest log lines that fit, `scroll` lines up from the end.
fn draw_log(canvas: &mut Canvas, area: Rect, scroll: usize, lines: &[String]) {
    let height = usize::from(area.height);
    let last = lines
        .len()
        .saturating_sub(scroll)
        .max(height.min(lines.len()));
    let visible = lines[last.saturating_sub(height)..last].iter().map(|line| {
        let style = if line.contains(" ERROR ") {
            Style::new().fg(Color::Red)
        } else if line.contains(" WARN ") {
            Style::new().fg(Color::Yellow)
        } else if line.contains(" DEBUG ") || line.contains(" TRACE ") {
            dim()
        } else {
            Style::new()
        };
        Line::styled(format!(" {line}"), style)
    });
    canvas.render_widget(Paragraph::new(visible.collect::<Vec<_>>()), area);
}

fn popup(canvas: &mut Canvas, area: Rect, title: &'static str, lines: Vec<Line<'static>>) {
    let widest = lines.iter().map(Line::width).max().unwrap_or(0);
    let width = (widest as u16 + 4).clamp(30, area.width);
    let height = (lines.len() as u16 + 4).min(area.height);
    let place = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let mut padded = vec![Line::raw("")];
    padded.extend(lines.into_iter().map(|line| {
        let mut spans = vec![Span::raw(" ")];
        spans.extend(line.spans);
        Line::from(spans)
    }));
    canvas.render_widget(Clear, place);
    canvas.render_widget(
        Paragraph::new(padded).block(Block::bordered().title_top(title)),
        place,
    );
}

fn draw_dialog(canvas: &mut Canvas, area: Rect, dialog: &AlarmDialog, snapshot: &Snapshot) {
    let alarm = &dialog.alarm;
    let field = FIELDS[dialog.field];
    let value = |own: Field, text: String| {
        let style = if own == field {
            selected()
        } else {
            Style::new()
        };
        Span::styled(text, style)
    };
    let mut days = vec![Span::raw("Days     ")];
    for (day, initial) in ["S", "M", "T", "W", "T", "F", "S"].iter().enumerate() {
        let chosen = alarm.days.0 & (1 << day) != 0;
        let text = if chosen {
            format!("[{initial}]")
        } else {
            format!(" {initial} ")
        };
        let under_cursor = field == Field::Days && usize::from(dialog.day) == day;
        let style = if under_cursor {
            selected()
        } else {
            Style::new()
        };
        days.push(Span::styled(text, style));
    }
    let lines = vec![
        Line::from(vec![
            Span::raw("Time     "),
            value(Field::Hour, format!("{:02}", alarm.hour)),
            Span::raw(":"),
            value(Field::Minute, format!("{:02}", alarm.minute)),
        ]),
        Line::from(days),
        Line::from(vec![
            Span::raw("Sound    "),
            value(
                Field::Sound,
                format!("‹ {} ›", snapshot.alarm_sound_name(alarm.sound)),
            ),
        ]),
        Line::from(vec![
            Span::raw("Volume   "),
            value(
                Field::Volume,
                format!("{} {}", bar(alarm.volume, 100, 12), alarm.volume),
            ),
        ]),
        Line::from(vec![
            Span::raw("Enabled  "),
            value(
                Field::Enabled,
                if alarm.enabled { "[x]" } else { "[ ]" }.to_string(),
            ),
        ]),
        Line::raw(""),
        key_hints("tab next · h/l change · space toggle"),
        key_hints("enter save · esc cancel"),
    ];
    let title = if dialog.is_new {
        " New alarm "
    } else {
        " Edit alarm "
    };
    popup(canvas, area, title, lines);
}

type Attempt = Pin<Box<dyn Future<Output = Result<Case, case::Error>>>>;

enum Phase {
    Waiting(Instant),
    Connecting(Attempt),
    Up(Box<Case>),
}

/// Runs the interface until the user quits.
///
/// # Errors
///
/// Fails if the terminal cannot be drawn to.
pub async fn run() -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let outcome = session(&mut terminal).await;
    ratatui::restore();
    outcome
}

async fn session(terminal: &mut DefaultTerminal) -> anyhow::Result<()> {
    let (sender, mut events) = mpsc::unbounded_channel();
    // crossterm reads block, so they get a thread of their own.
    std::thread::spawn(move || {
        while let Ok(event) = event::read() {
            if sender.send(event).is_err() {
                break;
            }
        }
    });
    let mut app = App::default();
    let mut snapshot = Snapshot::default();
    let mut phase = Phase::Connecting(Box::pin(Case::connect()));
    let mut retry = FIRST_RETRY;
    let mut battery_due = interval_at(Instant::now() + BATTERY_PERIOD, BATTERY_PERIOD);

    loop {
        terminal.draw(|canvas| draw(canvas, &app, &snapshot))?;
        phase = match phase {
            Phase::Waiting(until) => tokio::select! {
                () = sleep_until(until) => {
                    app.link = Link::Connecting;
                    Phase::Connecting(Box::pin(Case::connect()))
                }
                event = events.recv() => match action_for(&mut app, &snapshot, event) {
                    Action::Quit => return Ok(()),
                    Action::Reconnect => {
                        app.link = Link::Connecting;
                        Phase::Connecting(Box::pin(Case::connect()))
                    }
                    Action::ConnectAudio => {
                        connect_audio(&mut app, terminal, &snapshot).await?;
                        app.link = Link::Connecting;
                        Phase::Connecting(Box::pin(Case::connect()))
                    }
                    _ => Phase::Waiting(until),
                },
            },
            Phase::Connecting(mut attempt) => tokio::select! {
                connected = &mut attempt => match connected {
                    Ok(case) => {
                        app.link = Link::Up;
                        app.notice = None;
                        retry = FIRST_RETRY;
                        let survey = Snapshot::survey().to_vec();
                        match send_all(&case, &mut snapshot, &mut app, survey).await {
                            Ok(()) => Phase::Up(Box::new(case)),
                            Err(error) => dropped(&mut app, &error, FIRST_RETRY),
                        }
                    }
                    Err(error) => {
                        let wait = retry;
                        retry = (retry * 2).min(LONGEST_RETRY);
                        dropped(&mut app, &error, wait)
                    }
                },
                event = events.recv() => match action_for(&mut app, &snapshot, event) {
                    Action::Quit => return Ok(()),
                    Action::ConnectAudio => {
                        connect_audio(&mut app, terminal, &snapshot).await?;
                        Phase::Connecting(attempt)
                    }
                    _ => Phase::Connecting(attempt),
                },
            },
            Phase::Up(case) => {
                let outcome = tokio::select! {
                    received = case.recv() => match received {
                        Ok(frame) => {
                            snapshot.apply(&frame);
                            on_push(&case, &mut snapshot, &mut app, &frame).await
                        }
                        Err(error) => Err(error),
                    },
                    event = events.recv() => match action_for(&mut app, &snapshot, event) {
                        Action::Quit => return Ok(()),
                        Action::Send(requests) => {
                            app.notice = None;
                            send_all(&case, &mut snapshot, &mut app, requests).await
                        }
                        Action::ConnectAudio => {
                            connect_audio(&mut app, terminal, &snapshot).await?;
                            Ok(())
                        }
                        _ => Ok(()),
                    },
                    _ = battery_due.tick() => {
                        let battery = vec![Frame::get(status::BATTERY_LEVEL)];
                        send_all(&case, &mut snapshot, &mut app, battery).await
                    }
                };
                match outcome {
                    Ok(()) => Phase::Up(case),
                    Err(error) => {
                        // Keys typed at a dead link would fire all at once on reconnect.
                        while events.try_recv().is_ok() {}
                        // Reconnecting at once would wake a case that just chose to sleep.
                        let went_idle = app.notice.as_deref() == Some(IDLE_NOTICE);
                        let wait = if went_idle {
                            LONGEST_RETRY
                        } else {
                            FIRST_RETRY
                        };
                        dropped(&mut app, &error, wait)
                    }
                }
            }
        };
    }
}

/// Brings up the audio link and reports the outcome in the notice line.
async fn connect_audio(
    app: &mut App,
    terminal: &mut DefaultTerminal,
    snapshot: &Snapshot,
) -> anyhow::Result<()> {
    app.notice = Some("connecting audio to this computer".to_string());
    terminal.draw(|canvas| draw(canvas, app, snapshot))?;
    app.notice = Some(match case::connect_audio().await {
        Ok(()) => "audio is connected to this computer".to_string(),
        Err(error) => format!("audio did not connect: {error}"),
    });
    Ok(())
}

fn dropped(app: &mut App, error: &case::Error, wait: Duration) -> Phase {
    log::info!("link down: {error}. Retrying in {wait:?}");
    app.link = Link::Down(error.to_string());
    Phase::Waiting(Instant::now() + wait)
}

fn action_for(app: &mut App, snapshot: &Snapshot, event: Option<Event>) -> Action {
    match event {
        Some(Event::Key(key)) => on_key(app, snapshot, key),
        Some(_) => Action::Stay,
        None => Action::Quit,
    }
}

/// Sends `requests` in order. A refusal becomes a notice. A dead link is an error.
async fn send_all(
    case: &Case,
    snapshot: &mut Snapshot,
    app: &mut App,
    requests: Vec<Frame>,
) -> Result<(), case::Error> {
    for request in requests {
        match case.exchange(&request, |frame| snapshot.apply(frame)).await {
            Ok(_) => {}
            Err(error @ (case::Error::Refused(..) | case::Error::Hazardous(_))) => {
                log::warn!("{error}");
                app.notice = Some(error.to_string());
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

/// Words for the case's announcement that it is about to drop a host.
///
/// The payload is a reason code, then the Bluetooth address of that host.
fn disconnect_notice(frame: &Frame) -> Option<String> {
    if frame.function != DISCONNECT || frame.operator != Operator::Processing {
        return None;
    }
    Some(match frame.payload.first() {
        Some(34) => IDLE_NOTICE.to_string(),
        Some(35) => "Bluetooth mode changed".to_string(),
        Some(code) => format!("the case is disconnecting (reason {code})"),
        None => "the case is disconnecting".to_string(),
    })
}

/// Reacts to a frame the case sent unprompted.
async fn on_push(
    case: &Case,
    snapshot: &mut Snapshot,
    app: &mut App,
    frame: &Frame,
) -> Result<(), case::Error> {
    if let Some(reason) = disconnect_notice(frame) {
        log::info!("{reason}");
        app.notice = Some(reason);
    }
    // A bud entering or leaving the case changes its charge reading soon after.
    // The case pushes battery levels itself alongside its charging state.
    if frame.operator == Operator::Status && frame.function == status::BUDS_LOCATION {
        let battery = vec![Frame::get(status::BATTERY_LEVEL)];
        return send_all(case, snapshot, app, battery).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    const SESSION: &str = "\
        02 02 03 06 64 5b 0f 00 00 00
        02 08 03 01 01
        02 0f 03 01 01
        05 07 03 03 12 01 00
        05 09 03 02 01 28
        01 0d 03 02 00 00
        01 10 03 01 00
        01 17 03 03 01 00 00
        01 07 03 0c 02 06 02 4d 36 01 12 03 4d 31 38 01
        01 12 03 09 02 08 02 41 38 04 02 41 34
        01 06 03 0f 02 00 06 2d 01 64 3e 08 02 09 00 00 64 41 08";

    fn replayed() -> Snapshot {
        let mut snapshot = Snapshot::default();
        for line in SESSION.lines() {
            snapshot.apply(&line.parse().unwrap());
        }
        snapshot
    }

    fn press(app: &mut App, snapshot: &Snapshot, code: KeyCode) -> Action {
        on_key(app, snapshot, KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn sent(action: Action) -> Vec<String> {
        match action {
            Action::Send(requests) => requests.iter().map(Frame::to_string).collect(),
            other => panic!("expected requests, got {other:?}"),
        }
    }

    fn screen(app: &App, snapshot: &Snapshot, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|canvas| draw(canvas, app, snapshot)).unwrap();
        terminal.backend().to_string()
    }

    #[test]
    fn space_pauses_a_playing_sound() {
        let action = press(&mut App::default(), &replayed(), KeyCode::Char(' '));
        assert_eq!(sent(action), ["05 07 02 03 12 02 00"]);
    }

    #[test]
    fn plus_and_minus_step_the_volume() {
        let snapshot = replayed();
        let mut app = App::default();
        assert_eq!(
            sent(press(&mut app, &snapshot, KeyCode::Char('+'))),
            ["05 09 02 01 2d"]
        );
        assert_eq!(
            sent(press(&mut app, &snapshot, KeyCode::Char('-'))),
            ["05 09 02 01 23"]
        );
    }

    #[test]
    fn enter_on_another_sound_pauses_selects_and_plays() {
        let snapshot = replayed();
        let mut app = App::default();
        let action = press(&mut app, &snapshot, KeyCode::Enter);
        assert_eq!(
            sent(action),
            [
                "05 07 02 03 12 02 00",
                "01 08 02 01 06",
                "05 07 02 03 06 01 00"
            ]
        );
    }

    #[test]
    fn the_cursor_stops_at_the_last_row() {
        let snapshot = replayed();
        let mut app = App::default();
        for _ in 0..5 {
            press(&mut app, &snapshot, KeyCode::Char('j'));
        }
        assert_eq!(app.sound_row, 1);
        press(&mut app, &snapshot, KeyCode::Char('k'));
        press(&mut app, &snapshot, KeyCode::Char('k'));
        assert_eq!(app.sound_row, 0);
    }

    #[test]
    fn enter_on_an_alarm_sets_the_clock_then_toggles_it() {
        let snapshot = replayed();
        let mut app = App::default();
        press(&mut app, &snapshot, KeyCode::Char('2'));
        let requests = sent(press(&mut app, &snapshot, KeyCode::Enter));
        assert!(requests[0].starts_with("01 16 02 07"));
        assert_eq!(requests[1], "01 06 02 07 00 06 2d 00 64 3e 08");
    }

    #[test]
    fn deleting_an_alarm_needs_confirmation() {
        let snapshot = replayed();
        let mut app = App::default();
        press(&mut app, &snapshot, KeyCode::Char('2'));
        press(&mut app, &snapshot, KeyCode::Char('d'));
        assert_eq!(press(&mut app, &snapshot, KeyCode::Char('n')), Action::Stay);
        press(&mut app, &snapshot, KeyCode::Char('d'));
        let requests = sent(press(&mut app, &snapshot, KeyCode::Char('y')));
        assert_eq!(requests, ["01 13 02 01 00", "01 06 01 00"]);
    }

    #[test]
    fn the_dialog_edits_then_saves_a_new_alarm() {
        let snapshot = replayed();
        let mut app = App::default();
        press(&mut app, &snapshot, KeyCode::Char('2'));
        press(&mut app, &snapshot, KeyCode::Char('a'));
        press(&mut app, &snapshot, KeyCode::Char('l'));
        press(&mut app, &snapshot, KeyCode::Tab);
        press(&mut app, &snapshot, KeyCode::Char('l'));
        press(&mut app, &snapshot, KeyCode::Tab);
        press(&mut app, &snapshot, KeyCode::Char(' '));
        let requests = sent(press(&mut app, &snapshot, KeyCode::Enter));
        // Slot 1, 08:05, enabled, volume 60, every day but Monday, sound 8.
        assert_eq!(requests[1], "01 06 02 07 01 08 05 01 3c 7d 08");
        assert_eq!(app.dialog, None);
    }

    #[test]
    fn escape_discards_the_dialog() {
        let snapshot = replayed();
        let mut app = App::default();
        press(&mut app, &snapshot, KeyCode::Char('2'));
        press(&mut app, &snapshot, KeyCode::Char('e'));
        assert_eq!(press(&mut app, &snapshot, KeyCode::Esc), Action::Stay);
        assert_eq!(app.dialog, None);
    }

    #[test]
    fn settings_keys_change_the_selected_setting() {
        let snapshot = replayed();
        let mut app = App::default();
        press(&mut app, &snapshot, KeyCode::Char('3'));
        assert_eq!(
            sent(press(&mut app, &snapshot, KeyCode::Char('l'))),
            ["01 17 02 01 02", "01 17 01 00"]
        );
        press(&mut app, &snapshot, KeyCode::Char('j'));
        assert_eq!(
            sent(press(&mut app, &snapshot, KeyCode::Char('l'))),
            ["01 10 02 01 01"]
        );
        press(&mut app, &snapshot, KeyCode::Char('j'));
        assert_eq!(
            sent(press(&mut app, &snapshot, KeyCode::Char('l'))),
            ["01 0d 02 02 00 0f"]
        );
    }

    #[test]
    fn q_quits_and_c_reconnects() {
        let snapshot = replayed();
        let mut app = App::default();
        assert_eq!(
            press(&mut app, &snapshot, KeyCode::Char('c')),
            Action::Reconnect
        );
        assert_eq!(
            press(&mut app, &snapshot, KeyCode::Char('A')),
            Action::ConnectAudio
        );
        assert_eq!(press(&mut app, &snapshot, KeyCode::Char('q')), Action::Quit);
    }

    #[test]
    fn the_sounds_tab_shows_the_header_and_the_list() {
        let app = App {
            link: Link::Up,
            ..App::default()
        };
        let screen = screen(&app, &replayed(), 80, 24);
        assert!(screen.contains("● connected · lid open"));
        assert!(screen.contains("Case ████████ 100%"));
        assert!(screen.contains("Right █░░░░░░░ 15%"));
        assert!(screen.contains("▶ Intimate Rain   Volume ████░░░░░░░░ 40   Timer off"));
        assert!(screen.contains("[1 Sounds]"));
        assert!(screen.contains("Rainfall"));
        assert!(screen.contains("q quit"));
    }

    #[test]
    fn the_alarms_tab_lists_alarms_and_the_dialog() {
        let snapshot = replayed();
        let mut app = App::default();
        press(&mut app, &snapshot, KeyCode::Char('2'));
        let screen_before = screen(&app, &snapshot, 80, 24);
        assert!(screen_before.contains("06:45   weekdays"));
        assert!(screen_before.contains("Wake Up"));
        press(&mut app, &snapshot, KeyCode::Char('e'));
        let screen_after = screen(&app, &snapshot, 80, 24);
        assert!(screen_after.contains("Edit alarm"));
        assert!(screen_after.contains("[M][T][W][T][F]"));
    }

    #[test]
    fn the_log_tab_follows_the_newest_lines_and_scrolls_back() {
        let lines: Vec<String> = (1..=40)
            .map(|n| format!("03:00:00 INFO line {n}"))
            .collect();
        let shown = |scroll| {
            let mut terminal = Terminal::new(TestBackend::new(40, 5)).unwrap();
            terminal
                .draw(|canvas| draw_log(canvas, canvas.area(), scroll, &lines))
                .unwrap();
            terminal.backend().to_string()
        };
        assert!(shown(0).contains("line 40"));
        assert!(shown(0).contains("line 36"));
        assert!(!shown(0).contains("line 35"));
        assert!(shown(10).contains("line 30"));
        assert!(!shown(10).contains("line 31"));
        assert!(shown(usize::MAX).contains("line 1 "));
        assert!(shown(usize::MAX).contains("line 5"));
    }

    #[test]
    fn log_scrolling_is_bounded_by_the_line_count() {
        assert_eq!(scrolled(0, -1, 40), 1);
        assert_eq!(scrolled(2, 1, 40), 1);
        assert_eq!(scrolled(0, 1, 40), 0);
        assert_eq!(scrolled(usize::MAX, 1, 40), 39);
        assert_eq!(scrolled(5, LOG_PAGE_LINES, 40), 0);
        assert_eq!(scrolled(5, -LOG_PAGE_LINES, 40), 15);
    }

    #[test]
    fn capital_g_returns_the_log_to_following() {
        let snapshot = replayed();
        let mut app = App::default();
        press(&mut app, &snapshot, KeyCode::Char('5'));
        press(&mut app, &snapshot, KeyCode::Char('g'));
        assert_eq!(app.log_scroll, usize::MAX);
        press(&mut app, &snapshot, KeyCode::Char('G'));
        assert_eq!(app.log_scroll, 0);
    }

    #[test]
    fn the_disconnect_reason_is_the_first_payload_byte() {
        // Captured after the lid closed: reason 34, then the host's address.
        let idle: Frame = "04 02 07 07 22 66 55 44 33 22 11".parse().unwrap();
        assert_eq!(disconnect_notice(&idle).as_deref(), Some(IDLE_NOTICE));
        let other: Frame = "04 02 07 01 06".parse().unwrap();
        assert_eq!(
            disconnect_notice(&other).as_deref(),
            Some("the case is disconnecting (reason 6)")
        );
        let unrelated: Frame = "02 0f 03 01 00".parse().unwrap();
        assert_eq!(disconnect_notice(&unrelated), None);
    }

    #[test]
    fn a_small_terminal_gets_a_plain_message() {
        let screen = screen(&App::default(), &replayed(), 60, 10);
        assert!(screen.contains("terminal too small"));
    }

    #[test]
    fn a_dropped_link_shows_its_reason() {
        let app = App {
            link: Link::Down("the case is not reachable".to_string()),
            ..App::default()
        };
        assert!(screen(&app, &replayed(), 80, 24).contains("○ the case is not reachable"));
    }
}
