use crate::{
    dacp::Command,
    settings::{Dialog, Field, Input, Mode},
    spectrum::{BAND_COUNT, Spectrum},
    state::{Player, PresenceStatus, Visualizer, format_time},
};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use image::{DynamicImage, imageops::FilterType};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::{
        Block, BorderType, Clear, Gauge, Paragraph, Wrap,
        canvas::{Canvas, Line as CanvasLine},
    },
};
use std::{borrow::Cow, f64::consts::TAU, ops::Range, time::Instant};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

const BACKGROUND: Color = Color::Rgb(16, 22, 27);
const SURFACE: Color = Color::Rgb(24, 33, 39);
const BORDER: Color = Color::Rgb(53, 69, 77);
const TEXT: Color = Color::Rgb(233, 239, 240);
const MUTED: Color = Color::Rgb(157, 177, 182);
const DIM: Color = Color::Rgb(111, 129, 136);
const ACCENT: Color = Color::Rgb(133, 232, 188);
const WARNING: Color = Color::Rgb(241, 190, 128);
const SPECTRUM_BACKGROUND: Color = Color::Rgb(14, 10, 27);
const SPECTRUM_GRID: Color = Color::Rgb(61, 47, 86);
const SPECTRUM_LOW: Color = Color::Rgb(89, 241, 130);
const SPECTRUM_MID: Color = Color::Rgb(226, 243, 107);
const SPECTRUM_HIGH: Color = Color::Rgb(255, 169, 66);
const SPECTRUM_PEAK: Color = Color::Rgb(255, 108, 193);
const LYRIC_SYNC_STEP_MS: i64 = 250;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Transport(Command),
    Discord,
    CycleVisualizer,
    OpenSettings,
    Settings(Input),
    Scroll(i8),
    AdjustLyricSync(i64),
    Quit,
}

const BUTTON_ACTIONS: [Action; 4] = [
    Action::Transport(Command::Previous),
    Action::Transport(Command::PlayPause),
    Action::Transport(Command::Next),
    Action::Discord,
];

pub fn key_action(code: KeyCode) -> Option<Action> {
    match code {
        KeyCode::Char('p') => Some(BUTTON_ACTIONS[0]),
        KeyCode::Char(' ') => Some(BUTTON_ACTIONS[1]),
        KeyCode::Char('n') => Some(BUTTON_ACTIONS[2]),
        KeyCode::Char('d') => Some(Action::Discord),
        KeyCode::Char('v') => Some(Action::CycleVisualizer),
        KeyCode::Char('s') => Some(Action::OpenSettings),
        KeyCode::Up => Some(Action::Scroll(-1)),
        KeyCode::Down => Some(Action::Scroll(1)),
        KeyCode::Char('[') => Some(Action::AdjustLyricSync(-LYRIC_SYNC_STEP_MS)),
        KeyCode::Char(']') => Some(Action::AdjustLyricSync(LYRIC_SYNC_STEP_MS)),
        KeyCode::Char('q') => Some(Action::Quit),
        _ => None,
    }
}

#[derive(Default)]
pub struct Screen {
    pub buttons: [Rect; 4],
    pub cover: Option<Rect>,
    pub scroll_top: usize,
    pub scroll_max: usize,
    transport: bool,
    settings: Rect,
    modal: Option<SettingsScreen>,
}

struct SettingsScreen {
    area: Rect,
    targets: [Option<(Rect, Input)>; 5],
}

impl Screen {
    pub fn click(&self, x: u16, y: u16) -> Option<Action> {
        let position = Position::new(x, y);
        if let Some(modal) = &self.modal {
            if !modal.area.contains(position) {
                return None;
            }
            return modal.targets.iter().flatten().find_map(|&(area, input)| {
                area.contains(position).then_some(Action::Settings(input))
            });
        }
        if self.settings.contains(position) {
            return Some(Action::OpenSettings);
        }
        let index = self
            .buttons
            .iter()
            .position(|area| area.contains(position))?;
        (self.transport || index == 3).then_some(BUTTON_ACTIONS[index])
    }

    pub fn key(&self, key: KeyEvent) -> Option<Action> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        if key.kind == KeyEventKind::Press
            && key.modifiers == KeyModifiers::CONTROL
            && key.code == KeyCode::Char('c')
        {
            return Some(Action::Quit);
        }
        if self.modal.is_some() {
            return Dialog::key(key)
                .filter(|input| {
                    key.kind == KeyEventKind::Press
                        || matches!(input, Input::Character(_) | Input::Backspace)
                })
                .map(Action::Settings);
        }
        if key.modifiers != KeyModifiers::NONE
            || (key.kind == KeyEventKind::Repeat
                && !matches!(key.code, KeyCode::Up | KeyCode::Down))
        {
            return None;
        }
        key_action(key.code)
            .filter(|action| self.transport || !matches!(action, Action::Transport(_)))
    }
}

pub fn draw(
    frame: &mut Frame,
    player: &Player,
    name: &str,
    native_graphics: bool,
    now: Instant,
    dialog: Option<&Dialog>,
) -> Screen {
    let area = frame.area();
    let mut screen = Screen {
        transport: player.transport,
        ..Screen::default()
    };
    if area.is_empty() {
        return screen;
    }
    if let Some(dialog) = dialog
        && (area.width < 62 || area.height < 18)
    {
        screen.modal = Some(draw_settings(
            frame,
            dialog,
            area,
            player.receiver_unavailable,
        ));
        return screen;
    }

    frame.render_widget(
        Block::new().style(Style::new().bg(BACKGROUND).fg(TEXT)),
        area,
    );
    let margin = u16::from(area.width >= 40);
    let area = Rect::new(
        area.x + margin,
        area.y,
        area.width - 2 * margin,
        area.height,
    );
    let hint_height = u16::from(area.height >= 12);
    let [header, body, progress, footer, hints] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(hint_height),
    ])
    .areas(area);

    screen.settings = header_line(frame, player, name, header);
    draw_progress(frame, player, progress);
    screen.buttons[3] = draw_discord(frame, player, footer);
    let help = if player.connected && !player.transport && hints.width >= 80 {
        "v View  ↑↓ Scroll  [] Sync  d Discord  q Quit  ·  Sender controls unavailable"
    } else if hints.width >= 68 {
        "p Space n Playback  v View  ↑↓ Scroll  [] Sync  d Discord  q Quit"
    } else {
        "v View  q Quit  ↑↓ Scroll  [] Sync  d Discord"
    };
    frame.render_widget(Paragraph::new(help).style(Style::new().fg(MUTED)), hints);

    let wide = body.width >= 66 && body.height >= 10;
    let (track, lyrics) = if wide {
        let [track, _, lyrics] = Layout::horizontal([
            Constraint::Percentage(45),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(body);
        (track, lyrics)
    } else {
        let track_height = if player.visualizer.is_some() && body.height >= 10 {
            (body.height / 2).clamp(7, 11)
        } else {
            6
        };
        let [track, lyrics] =
            Layout::vertical([Constraint::Length(track_height), Constraint::Min(0)]).areas(body);
        (track, lyrics)
    };

    let (cover, buttons) = draw_track(frame, player, name, track, wide, native_graphics);
    screen.cover = cover;
    screen.buttons[..3].copy_from_slice(&buttons);
    (screen.scroll_top, screen.scroll_max) = draw_lyrics(frame, player, lyrics, now);
    if let Some(dialog) = dialog {
        let area = frame.area();
        let area = Rect::new(
            area.x + (area.width - 60) / 2,
            area.y + (area.height - 16) / 2,
            60,
            16,
        );
        screen.modal = Some(draw_settings(
            frame,
            dialog,
            area,
            player.receiver_unavailable,
        ));
        screen.cover = None;
    }
    screen
}

fn header_line(frame: &mut Frame, player: &Player, name: &str, area: Rect) -> Rect {
    let (status, color) = if player.receiver_unavailable {
        ("Receiver unavailable", WARNING)
    } else if player.connected {
        ("Connected", ACCENT)
    } else if player.ready {
        ("Ready", MUTED)
    } else {
        ("Starting UxPlay", WARNING)
    };
    let settings_label = if area.width >= 16 {
        " Settings "
    } else {
        " s "
    };
    let [label, status_area, _, settings] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(
            (status.width() as u16).min(area.width.saturating_sub(settings_label.len() as u16 + 1)),
        ),
        Constraint::Length(1),
        Constraint::Length(settings_label.len() as u16),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(if label.width >= 42 {
            Line::from(vec![
                Span::styled("AirPlay", Style::new().fg(ACCENT).bold()),
                Span::styled(format!("  /  {name}"), Style::new().fg(TEXT)),
            ])
        } else {
            Line::styled(name, Style::new().fg(TEXT).bold())
        }),
        label,
    );
    frame.render_widget(
        Paragraph::new(status)
            .style(Style::new().fg(color))
            .right_aligned(),
        status_area,
    );
    frame.render_widget(
        Paragraph::new(settings_label).style(Style::new().fg(TEXT).bg(BORDER)),
        settings,
    );
    settings
}

fn card(title: impl Into<Line<'static>>) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER))
        .title(title.into().style(Style::new().fg(MUTED)))
        .style(Style::new().bg(SURFACE).fg(TEXT))
}

fn draw_settings(
    frame: &mut Frame,
    dialog: &Dialog,
    area: Rect,
    unavailable: bool,
) -> SettingsScreen {
    let mut screen = SettingsScreen {
        area,
        targets: [None; 5],
    };
    frame.render_widget(Clear, area);
    let content = if area.width >= 12 && area.height >= 8 {
        let shell = card(" Settings ");
        let content = shell.inner(area);
        frame.render_widget(shell, area);
        content
    } else {
        frame.render_widget(Block::new().style(Style::new().bg(SURFACE).fg(TEXT)), area);
        area
    };
    if content.is_empty() {
        return screen;
    }
    let inset = u16::from(content.width >= 30);
    let content = Rect::new(
        content.x + inset,
        content.y,
        content.width - 2 * inset,
        content.height,
    );
    let help = u16::from(content.height >= 7);
    match dialog.mode {
        Mode::Edit => {
            let text_height = if content.height >= 9 { 2 } else { 1 };
            let [name, lyrics, view, discord, notice, save, hints] = Layout::vertical([
                Constraint::Length(text_height),
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(text_height),
                Constraint::Fill(1),
                Constraint::Length(1),
                Constraint::Length(help),
            ])
            .areas(content);
            draw_settings_text_field(
                frame,
                dialog,
                name,
                Field::Name,
                if dialog.locked(Field::Name) {
                    "Speaker name (--name)"
                } else {
                    "Speaker name"
                },
                dialog.name(),
            );
            draw_settings_text_field(
                frame,
                dialog,
                discord,
                Field::DiscordAppId,
                if dialog.locked(Field::DiscordAppId) {
                    "Discord ID (--discord-app-id)"
                } else if text_height == 1 {
                    "Discord ID"
                } else {
                    "Discord Application ID"
                },
                dialog.discord_app_id().unwrap_or_default(),
            );
            let lyrics_value = if dialog.online_lyrics() { "On" } else { "Off" };
            let lyrics_label = if dialog.locked(Field::Lyrics) {
                format!("Online lyrics  {lyrics_value}  (--no-online-lyrics)")
            } else {
                format!("Online lyrics  {lyrics_value} ›")
            };
            for (field, label, row) in [
                (Field::Lyrics, lyrics_label, lyrics),
                (
                    Field::View,
                    format!("Default view   {} ›", dialog.draft.view.label()),
                    view,
                ),
            ] {
                frame.render_widget(
                    Paragraph::new(label)
                        .style(option_style(dialog.focus == field, dialog.locked(field))),
                    row,
                );
            }
            draw_notice(frame, dialog, unavailable, notice);
            let save = dialog_button(
                frame,
                save,
                if unavailable {
                    " Save & retry "
                } else {
                    " Save "
                },
                dialog.focus == Field::Save,
            );
            frame.render_widget(
                Paragraph::new("Tab/Shift+Tab · Enter · Esc").style(Style::new().fg(MUTED)),
                hints,
            );
            screen.targets = [
                (name, Field::Name),
                (lyrics, Field::Lyrics),
                (view, Field::View),
                (discord, Field::DiscordAppId),
                (save, Field::Save),
            ]
            .map(|(area, field)| (!dialog.locked(field)).then_some((area, Input::Activate(field))));
        }
        Mode::Confirm { rename } => {
            let [message, buttons, hints] = Layout::vertical([
                Constraint::Fill(1),
                Constraint::Length(1),
                Constraint::Length(help),
            ])
            .areas(content);
            frame.render_widget(
                Paragraph::new(format!(
                    "Rename to {}?\nThe connected sender will be disconnected.",
                    dialog.name().trim()
                ))
                .wrap(Wrap { trim: true }),
                message,
            );
            let [_, cancel, _, confirm] = Layout::horizontal([
                Constraint::Fill(1),
                Constraint::Length(8),
                Constraint::Length(1),
                Constraint::Length(8),
            ])
            .areas(buttons);
            screen.targets[0] = Some((
                dialog_button(frame, cancel, " Cancel ", !rename),
                Input::Confirm(false),
            ));
            screen.targets[1] = Some((
                dialog_button(frame, confirm, " Rename ", rename),
                Input::Confirm(true),
            ));
            frame.render_widget(
                Paragraph::new("Tab/Shift+Tab · Enter · Esc").style(Style::new().fg(MUTED)),
                hints,
            );
        }
        Mode::Starting | Mode::Restoring => {
            let [message, notice, hints] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Fill(1),
                Constraint::Length(help),
            ])
            .areas(content);
            let text = if dialog.mode == Mode::Starting {
                "Waiting for AirPlay…"
            } else {
                "Restoring the previous receiver…"
            };
            frame.render_widget(Paragraph::new(text).style(Style::new().fg(MUTED)), message);
            draw_notice(frame, dialog, false, notice);
            let escape = if dialog.mode == Mode::Starting {
                "Esc · Cancel"
            } else {
                "Esc · Close"
            };
            frame.render_widget(Paragraph::new(escape).style(Style::new().fg(MUTED)), hints);
        }
    }
    screen
}

fn draw_settings_text_field(
    frame: &mut Frame,
    dialog: &Dialog,
    area: Rect,
    field: Field,
    label: &str,
    mut value: &str,
) {
    let [label_area, input] = if area.height == 1 {
        Layout::horizontal([
            Constraint::Length((label.len() as u16 + 1).min(area.width / 2)),
            Constraint::Fill(1),
        ])
        .areas(area)
    } else {
        Layout::vertical([Constraint::Length(1); 2]).areas(area)
    };
    frame.render_widget(
        Paragraph::new(label).style(Style::new().fg(MUTED)),
        label_area,
    );
    let focused = dialog.focus == field;
    let selected = focused && dialog.text_selected;
    let style = if selected {
        Style::new().fg(BACKGROUND).bg(ACCENT)
    } else {
        option_style(focused, dialog.locked(field))
    };
    let cursor = if focused && !selected {
        let mut width = 1;
        let mut start = value.len();
        for (index, grapheme) in value.grapheme_indices(true).rev() {
            width += grapheme.width();
            if width > usize::from(input.width) {
                break;
            }
            start = index;
        }
        value = &value[start..];
        "▏"
    } else {
        if field == Field::DiscordAppId && value.is_empty() {
            value = "Not set";
        }
        ""
    };
    frame.render_widget(
        Paragraph::new(format!("{value}{cursor}")).style(style),
        input,
    );
}

fn draw_notice(frame: &mut Frame, dialog: &Dialog, unavailable: bool, area: Rect) {
    let mut lines = Vec::new();
    if unavailable {
        lines.push(Line::styled("Receiver unavailable", Style::new().bold()));
    }
    if let Some(error) = &dialog.error {
        lines.extend(error.lines().map(Line::from));
    }
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::new().fg(WARNING))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn option_style(focused: bool, locked: bool) -> Style {
    if focused {
        Style::new().fg(ACCENT).bg(BORDER).bold()
    } else if locked {
        Style::new().fg(MUTED)
    } else {
        Style::new().fg(TEXT)
    }
}

fn dialog_button(frame: &mut Frame, area: Rect, label: &str, focused: bool) -> Rect {
    let width = area.width.min(label.len() as u16);
    let area = Rect::new(area.right() - width, area.y, width, area.height);
    frame.render_widget(
        Paragraph::new(label).style(option_style(focused, false).bg(BORDER)),
        area,
    );
    area
}

fn draw_track(
    frame: &mut Frame,
    player: &Player,
    name: &str,
    area: Rect,
    wide: bool,
    native_graphics: bool,
) -> (Option<Rect>, [Rect; 3]) {
    let title = match player.visualizer {
        Some(Visualizer::Ribbons) => " Now Playing · Ribbons ",
        Some(Visualizer::Spectrum) => " Now Playing · Spectrum ",
        None => " Now Playing ",
    };
    let shell = card(title);
    let content = shell.inner(area);
    frame.render_widget(shell, area);
    if content.is_empty() {
        return (None, [Rect::ZERO; 3]);
    }

    let [media, controls, details] = if (wide || player.visualizer.is_some()) && content.height >= 5
    {
        Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(3),
        ])
        .areas(content)
    } else {
        let [details, controls] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(content);
        [Rect::ZERO, controls, details]
    };
    let lines = if let Some(track) = &player.track {
        vec![
            Line::styled(track.title.as_str(), Style::new().fg(TEXT).bold()),
            Line::styled(track.artist.as_str(), Style::new().fg(ACCENT)),
            Line::styled(track.album.as_str(), Style::new().fg(MUTED)),
        ]
    } else {
        let message = if player.connected {
            Cow::Borrowed("Waiting for track details")
        } else if player.reconnect {
            Cow::Owned(format!("Reconnect to {name} in Apple Music"))
        } else if player.receiver_unavailable {
            Cow::Borrowed("Open Settings to retry")
        } else {
            Cow::Borrowed("Select receiver in Apple Music")
        };
        vec![Line::styled(message, Style::new().fg(MUTED))]
    };
    let details_text = Paragraph::new(lines).centered();
    let details_text = if player.track.is_none() {
        details_text.wrap(Wrap { trim: true })
    } else {
        details_text
    };
    frame.render_widget(details_text, details);

    let buttons = draw_transport(frame, player.transport, controls);
    if let Some(visualizer) = player.visualizer {
        draw_visualizer(frame, media, visualizer, player.spectrum.as_ref());
        return (None, buttons);
    }
    if media.height < 3 {
        return (None, buttons);
    }
    if let Some(image) = &player.artwork {
        let cover = fit_cover(media, image);
        if native_graphics {
            return (Some(cover), buttons);
        }
        draw_half_blocks(frame, image, cover);
    } else if player.track.is_some() {
        let line = Rect::new(media.x, media.bottom() - 1, media.width, 1);
        frame.render_widget(
            Paragraph::new("No artwork")
                .style(Style::new().fg(DIM))
                .centered(),
            line,
        );
    }
    (None, buttons)
}

fn draw_visualizer(
    frame: &mut Frame,
    area: Rect,
    visualizer: Visualizer,
    spectrum: Option<&Spectrum>,
) {
    if area.is_empty() {
        return;
    }
    let inset = u16::from(area.width > 8);
    let width = area.width - 2 * inset;
    let plot_width = width.min(3 * BAND_COUNT as u16);
    let chart_height = area.height.min(plot_width.div_ceil(2));
    let area = Rect::new(
        area.x + (area.width - plot_width) / 2,
        area.bottom() - chart_height,
        plot_width,
        chart_height,
    );
    frame.render_widget(
        Block::new().style(Style::new().bg(SPECTRUM_BACKGROUND)),
        area,
    );
    let Some(spectrum) = spectrum else {
        let message = Rect::new(area.x, area.y + area.height / 2, area.width, 1);
        frame.render_widget(
            Paragraph::new("Waiting for audio")
                .style(Style::new().fg(MUTED))
                .centered(),
            message,
        );
        return;
    };

    match visualizer {
        Visualizer::Ribbons => draw_ribbons(frame, area, spectrum),
        Visualizer::Spectrum => draw_spectrum(frame, area, spectrum),
    }
}

fn draw_ribbons(frame: &mut Frame, area: Rect, spectrum: &Spectrum) {
    let strongest = |levels: &[f32]| f64::from(levels.iter().copied().fold(0.0, f32::max));
    let bass = strongest(&spectrum.levels[..8]);
    let treble = strongest(&spectrum.levels[16..]);
    let strength = strongest(&spectrum.levels);
    let rotation = f64::from(spectrum.phase);
    let aspect = (f64::from(area.width) / (2.0 * f64::from(area.height))).max(1.0);
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .background_color(SPECTRUM_BACKGROUND)
        .x_bounds([-aspect, aspect])
        .y_bounds([-1.0, 1.0])
        .paint(|context| {
            // Paint older traces first, then both bright edges of each ribbon.
            for (lag, edge, blue, rose) in [
                (0.55, -0.025, Color::Rgb(42, 30, 94), Color::Rgb(88, 19, 63)),
                (0.27, 0.0, Color::Rgb(93, 63, 204), Color::Rgb(161, 34, 111)),
                (
                    0.0,
                    0.035,
                    Color::Rgb(109, 113, 254),
                    Color::Rgb(253, 73, 148),
                ),
                (
                    0.0,
                    -0.018,
                    Color::Rgb(113, 243, 255),
                    Color::Rgb(255, 174, 103),
                ),
            ] {
                let phase = rotation - lag;
                for (turn, folds, color) in [(phase, 3.0, blue), (2.2 - phase, 4.0, rose)] {
                    let point = |angle: f64| {
                        let ripple = (7.0 * angle + turn).sin() * 0.055 * treble;
                        let radius = 0.46
                            + 0.17 * bass
                            + edge
                            + ripple
                            + (0.13 + 0.13 * strength) * (folds * angle - turn).cos();
                        let warp = (2.0 * angle + turn).sin() * (0.20 + 0.11 * strength);
                        let (y, x) = (angle + turn + warp).sin_cos();
                        (
                            radius * x,
                            radius * y * (0.81 + 0.19 * (angle + turn).cos()),
                        )
                    };
                    const SEGMENTS: u32 = 96;
                    let (mut x, mut y) = point(0.0);
                    for segment in 1..=SEGMENTS {
                        let angle = TAU * f64::from(segment) / f64::from(SEGMENTS);
                        let (next_x, next_y) = point(angle);
                        context.draw(&CanvasLine::new(x, y, next_x, next_y, color));
                        (x, y) = (next_x, next_y);
                    }
                }
            }
        });
    frame.render_widget(canvas, area);
}

fn draw_spectrum(frame: &mut Frame, area: Rect, spectrum: &Spectrum) {
    let show_labels = area.width >= 22 && area.height >= 5;
    let plot_height = area.height - u16::from(show_labels);
    let columns = (area.width / 2).clamp(1, BAND_COUNT as u16);
    let stride = area.width / columns;
    let start_x = area.x + (area.width - stride * columns) / 2;
    let buffer = frame.buffer_mut();
    for column in 0..columns {
        let band = |column| usize::from(column) * BAND_COUNT / usize::from(columns);
        let bands = band(column)..band(column + 1);
        let height = |values: &[f32]| {
            let level = values[bands.clone()]
                .iter()
                .copied()
                .fold(0.0_f32, f32::max);
            (level * f32::from(plot_height)).round() as u16
        };
        let level = height(&spectrum.levels);
        let peak = height(&spectrum.peaks);
        for row in 0..plot_height {
            let (symbol, color) = if peak > 0 && row + 1 == peak {
                ("━", SPECTRUM_PEAK)
            } else if row < level {
                let color = match row * 3 / plot_height {
                    0 => SPECTRUM_LOW,
                    1 => SPECTRUM_MID,
                    _ => SPECTRUM_HIGH,
                };
                ("▄", color)
            } else if row % 3 == 0 {
                ("·", SPECTRUM_GRID)
            } else {
                continue;
            };
            let y = area.y + plot_height - row - 1;
            let x = start_x + column * stride;
            for x in x..x + stride.saturating_sub(1).max(1) {
                buffer[(x, y)].set_symbol(symbol).set_fg(color);
            }
        }
    }
    if show_labels {
        let y = area.bottom() - 1;
        let label = Style::new().fg(MUTED);
        buffer.set_string(area.x + 1, y, "50 Hz", label);
        buffer.set_string(area.right() - 7, y, "12 kHz", label);
    }
}

fn fit_cover(area: Rect, image: &DynamicImage) -> Rect {
    let ratio = 2.0 * f64::from(image.width()) / f64::from(image.height());
    let max_width = area.width.saturating_sub(2).clamp(1, 64);
    let max_height = area.height.clamp(1, 32);
    let width = max_width.min((f64::from(max_height) * ratio).round().max(1.0) as u16);
    let height = max_height.min((f64::from(width) / ratio).round().max(1.0) as u16);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.bottom() - height,
        width,
        height,
    )
}

fn draw_half_blocks(frame: &mut Frame, image: &DynamicImage, area: Rect) {
    let pixels = image
        .resize_exact(
            u32::from(area.width),
            u32::from(area.height) * 2,
            FilterType::Triangle,
        )
        .to_rgba8();
    for y in 0..area.height {
        for x in 0..area.width {
            let rgb = |row| {
                let [r, g, b, alpha] = pixels.get_pixel(u32::from(x), row).0;
                if alpha == 0 {
                    SURFACE
                } else {
                    Color::Rgb(r, g, b)
                }
            };
            let top = u32::from(y) * 2;
            frame.buffer_mut()[(area.x + x, area.y + y)]
                .set_symbol("▀")
                .set_fg(rgb(top))
                .set_bg(rgb(top + 1));
        }
    }
}

fn draw_lyrics(frame: &mut Frame, player: &Player, area: Rect, now: Instant) -> (usize, usize) {
    let view = player.lyric_view(now);
    let mut title = String::from(" Lyrics");
    if let Some(sync) = view.sync_label {
        title.push_str(" · ");
        title.push_str(sync);
    }
    if player.lyric_offset_ms != 0 {
        title.push_str(&format!(" {:+} ms", player.lyric_offset_ms));
    }
    title.push(' ');
    let shell = card(title);
    let mut inner = shell.inner(area);
    frame.render_widget(shell, area);
    if inner.height == 0 || inner.width == 0 {
        return (0, 0);
    }
    if inner.width > 8 {
        inner.x += 1;
        inner.width -= 2;
    }

    if let Some(message) = view.message {
        frame.render_widget(Paragraph::new(message).style(Style::new().fg(MUTED)), inner);
        return (0, 0);
    }

    let mut rows = Vec::new();
    let mut active_row = None;
    let normal = Style::new().fg(MUTED);
    let highlight_style = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
    for (index, lyric) in view.lines.into_iter().enumerate() {
        let active = view.active == Some(index);
        let focus = view
            .highlight
            .as_ref()
            .or(view.lit.last())
            .filter(|_| active);
        let lit = if active { view.lit.as_slice() } else { &[] };
        let (wrapped, focused_row) = wrap_lyric(
            lyric,
            usize::from(inner.width),
            normal,
            focus,
            lit,
            highlight_style,
        );
        if active {
            active_row = Some(rows.len() + focused_row.unwrap_or_default());
        }
        rows.extend(wrapped);
    }
    let height = usize::from(inner.height);
    let max = rows.len().saturating_sub(height);
    let top = view
        .scroll
        .unwrap_or_else(|| active_row.unwrap_or_default().saturating_sub(height / 2))
        .min(max);
    let visible: Vec<_> = rows.into_iter().skip(top).take(height).collect();
    frame.render_widget(Paragraph::new(visible), inner);
    (top, max)
}

fn wrap_lyric(
    text: &str,
    width: usize,
    normal: Style,
    focus: Option<&Range<usize>>,
    lit: &[Range<usize>],
    highlight_style: Style,
) -> (Vec<Line<'static>>, Option<usize>) {
    let mut rows = Vec::new();
    let mut row = Vec::new();
    let mut used = 0;
    let mut offset = 0;
    let mut focused_row = None;
    let mut lit = lit.iter().peekable();
    for segment in text.split_inclusive(char::is_whitespace) {
        let start = offset;
        offset += segment.len();
        let word = segment.trim_end_matches(char::is_whitespace);
        if word.is_empty() {
            continue;
        }
        let length = word.width();
        if used != 0 {
            if used + 1 + length > width {
                rows.push(Line::from(std::mem::take(&mut row)));
                used = 0;
            } else {
                push_lyric_span(&mut row, " ", normal);
                used += 1;
            }
        }
        for (index, grapheme) in word.grapheme_indices(true) {
            let cells = grapheme.width();
            if used > 0 && used + cells > width {
                rows.push(Line::from(std::mem::take(&mut row)));
                used = 0;
            }
            let position = start + index;
            let overlaps = |range: &Range<usize>| {
                position < range.end && position + grapheme.len() > range.start
            };
            if focus.is_some_and(overlaps) {
                focused_row.get_or_insert(rows.len());
            }
            while lit.peek().is_some_and(|range| range.end <= position) {
                lit.next();
            }
            let style = if lit.peek().is_some_and(|range| overlaps(range)) {
                highlight_style
            } else {
                normal
            };
            push_lyric_span(&mut row, grapheme, style);
            used += cells;
        }
    }
    rows.push(Line::from(row));
    (rows, focused_row)
}

fn push_lyric_span(row: &mut Vec<Span<'_>>, text: &str, style: Style) {
    if let Some(previous) = row.last_mut().filter(|span| span.style == style) {
        previous.content.to_mut().push_str(text);
    } else {
        row.push(Span::styled(text.to_owned(), style));
    }
}

#[cfg(test)]
#[test]
fn wide_layout_gives_album_art_room_to_grow() {
    use crate::state::Track;
    use ratatui::{Terminal, backend::TestBackend};

    let now = Instant::now();
    let mut player = Player::default();
    player.stage_artwork(Some(DynamicImage::new_rgb8(32, 32)), now);
    player.set_track(
        Some(Track {
            title: "Album Art".into(),
            ..Track::default()
        }),
        now,
    );

    for (columns, rows, minimum) in [(80, 24, 28), (100, 32, 40), (160, 50, 64)] {
        let mut terminal = Terminal::new(TestBackend::new(columns, rows)).unwrap();
        let mut screen = Screen::default();
        terminal
            .draw(|frame| screen = draw(frame, &player, "Test", true, now, None))
            .unwrap();
        let cover = screen.cover.unwrap();
        assert!(
            cover.width >= minimum,
            "{columns}×{rows} terminal used a {cover:?} cover"
        );
        assert!(cover.width.abs_diff(2 * cover.height) <= 1);

        terminal
            .draw(|frame| {
                draw(frame, &player, "Test", false, now, None);
            })
            .unwrap();
        let painted = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .filter(|cell| cell.symbol() == "▀")
            .count();
        assert_eq!(
            painted,
            usize::from(cover.width) * usize::from(cover.height)
        );
    }
}

#[cfg(test)]
#[test]
fn wrapped_lyrics_keep_word_highlighting_across_rows() {
    let normal = Style::new().fg(TEXT);
    let highlight = Style::new().fg(ACCENT).bold();
    let lit = "前".len().."前你🙂".len();
    let focus = "前你".len().."前你🙂".len();
    let (rows, focused_row) = wrap_lyric(
        "前你🙂後",
        4,
        normal,
        Some(&focus),
        std::slice::from_ref(&lit),
        highlight,
    );
    assert_eq!(focused_row, Some(1));
    assert_eq!(
        rows,
        [
            Line::from(vec![
                Span::styled("前", normal),
                Span::styled("你", highlight)
            ]),
            Line::from(vec![
                Span::styled("🙂", highlight),
                Span::styled("後", normal)
            ]),
        ]
    );
}

#[cfg(test)]
#[test]
fn settings_dialog_owns_input_and_uses_a_fullscreen_compact_layout() {
    use crate::settings::{Intent, Overrides, Store};
    use ratatui::{Terminal, backend::TestBackend};

    let now = Instant::now();
    let mut player = Player::default();
    player.transport = true;
    player.artwork = Some(DynamicImage::new_rgb8(10, 10));
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(
        Ok(directory.path().join("settings.json")),
        Overrides::default(),
    );
    let mut dialog = Dialog::new(&store);

    for (width, height) in [(100, 32), (38, 15)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut base = Screen::default();
        let mut overlay = Screen::default();
        terminal
            .draw(|frame| base = draw(frame, &player, "Home", true, now, None))
            .unwrap();
        assert_eq!(
            base.click(base.settings.x, base.settings.y),
            Some(Action::OpenSettings)
        );
        assert_eq!(
            base.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE)),
            Some(Action::OpenSettings)
        );
        terminal
            .draw(|frame| overlay = draw(frame, &player, "Home", true, now, Some(&dialog)))
            .unwrap();
        assert_eq!(
            overlay.modal.as_ref().unwrap().area == Rect::new(0, 0, width, height),
            width < 62
        );
        assert!(overlay.cover.is_none());
        assert_eq!(overlay.click(base.buttons[3].x, base.buttons[3].y), None);
        let (discord, _) = overlay
            .modal
            .as_ref()
            .unwrap()
            .targets
            .iter()
            .flatten()
            .find(|(_, input)| *input == Input::Activate(Field::DiscordAppId))
            .unwrap();
        assert!(!discord.is_empty());
        assert_eq!(
            overlay.click(discord.x, discord.y),
            Some(Action::Settings(Input::Activate(Field::DiscordAppId))),
        );
        for character in ['p', ' ', 'n'] {
            assert_eq!(
                overlay.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE)),
                Some(Action::Settings(Input::Character(character)))
            );
        }
        let mut press = |code, modifiers| {
            let Some(Action::Settings(input)) = overlay.key(KeyEvent::new(code, modifiers)) else {
                panic!("key did not reach Settings");
            };
            dialog.edit(input)
        };
        press(KeyCode::Char('音'), KeyModifiers::SHIFT);
        press(KeyCode::Char('楽'), KeyModifiers::NONE);
        press(KeyCode::Tab, KeyModifiers::NONE);
        press(KeyCode::Enter, KeyModifiers::NONE);
        press(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(
            press(KeyCode::Esc, KeyModifiers::NONE),
            Some(Intent::Dismiss)
        );
        assert_eq!(dialog.focus, Field::Name);
        assert_eq!(dialog.draft.name, "音楽");
        assert!(!dialog.online_lyrics());
        assert!(store.online_lyrics());
        dialog = Dialog::new(&store);
    }
}

fn draw_progress(frame: &mut Frame, player: &Player, area: Rect) {
    let (position, duration, ratio) = player.timing.map_or_else(
        || ("--:--".to_owned(), "--:--".to_owned(), 0.0),
        |timing| {
            (
                format_time(timing.position),
                format_time(timing.duration),
                f64::from(timing.position) / f64::from(timing.duration),
            )
        },
    );
    let label = format!(" {position} / {duration} ");
    frame.render_widget(
        Gauge::default()
            .ratio(ratio.clamp(0.0, 1.0))
            .label(label)
            .use_unicode(true)
            .style(Style::new().fg(TEXT).bg(SURFACE))
            .gauge_style(Style::new().fg(ACCENT).bg(SURFACE)),
        area,
    );
}

fn draw_transport(frame: &mut Frame, enabled: bool, area: Rect) -> [Rect; 3] {
    let labels = if area.width >= 30 {
        [" Previous ", " Play/Pause ", " Next "]
    } else if area.width >= 11 {
        [" ⏮ ", " ⏯ ", " ⏭ "]
    } else {
        ["⏮", "⏯", "⏭"]
    };
    let gap = u16::from(area.width >= 5);
    let width = labels.iter().map(|label| label.width() as u16).sum::<u16>() + 2 * gap;
    if area.height == 0 || width > area.width {
        return [Rect::ZERO; 3];
    }
    let mut x = area.x + (area.width - width) / 2;
    let style = if enabled {
        Style::new().fg(TEXT).bg(BORDER)
    } else {
        Style::new().fg(DIM).bg(BACKGROUND)
    };
    labels.map(|label| {
        let button = Rect::new(x, area.y, label.width() as u16, 1);
        frame.render_widget(Paragraph::new(label).style(style), button);
        x = button.right() + gap;
        button
    })
}

fn draw_discord(frame: &mut Frame, player: &Player, area: Rect) -> Rect {
    let label = format!(" Discord · {} ", player.discord.label());
    let color = match player.discord {
        PresenceStatus::Off => MUTED,
        PresenceStatus::Sharing => ACCENT,
        PresenceStatus::Unavailable => WARNING,
    };
    let area = Rect::new(
        area.x,
        area.y,
        area.width.min(label.width() as u16),
        area.height,
    );
    frame.render_widget(
        Paragraph::new(label).style(Style::new().fg(color).bg(SURFACE)),
        area,
    );
    area
}
