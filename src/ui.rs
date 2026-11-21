use crate::{
    dacp::Command,
    spectrum::{BAND_COUNT, Spectrum},
    state::{Player, PresenceStatus, Visualizer, format_time},
};
use crossterm::event::KeyCode;
use image::{DynamicImage, imageops::FilterType};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    symbols::Marker,
    text::{Line, Span},
    widgets::{
        Block, BorderType, Gauge, Paragraph,
        canvas::{Canvas, Line as CanvasLine},
    },
};
use std::{f64::consts::TAU, ops::Range, time::Instant};
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
}

impl Screen {
    pub fn click(&self, x: u16, y: u16) -> Option<Action> {
        let index = self
            .buttons
            .iter()
            .position(|area| area.contains(Position::new(x, y)))?;
        (self.transport || index == 3).then_some(BUTTON_ACTIONS[index])
    }
}

pub fn draw(
    frame: &mut Frame,
    player: &Player,
    name: &str,
    native_graphics: bool,
    now: Instant,
) -> Screen {
    let area = frame.area();
    let mut screen = Screen {
        transport: player.transport,
        ..Screen::default()
    };
    if area.is_empty() {
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
    let columns = match area.width {
        84.. => 4,
        44.. => 2,
        _ => 1,
    };
    let controls_height = 4 / columns;
    let hint_height = u16::from(area.height >= controls_height + 8);
    let [header, body, progress, buttons, hints] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(controls_height),
        Constraint::Length(hint_height),
    ])
    .areas(area);

    header_line(frame, player, name, header);
    draw_progress(frame, player, progress);
    screen.buttons = draw_buttons(frame, player, buttons, columns);
    let help = if player.connected && !player.transport && hints.width >= 80 {
        "v Visualizer  ↑↓ Scroll  [] Sync  d Discord  q Quit  ·  Sender controls unavailable"
    } else if hints.width >= 54 {
        "v Visualizer  ↑↓ Scroll  [] Sync  d Discord  q Quit"
    } else {
        "v Visualizer  q Quit  ↑↓ Scroll  [] Sync  d Discord"
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
            5
        };
        let [track, lyrics] =
            Layout::vertical([Constraint::Length(track_height), Constraint::Min(0)]).areas(body);
        (track, lyrics)
    };

    screen.cover = draw_track(frame, player, track, wide, native_graphics);
    (screen.scroll_top, screen.scroll_max) = draw_lyrics(frame, player, lyrics, now);
    screen
}

fn header_line(frame: &mut Frame, player: &Player, name: &str, area: Rect) {
    let (status, color) = if player.connected {
        ("Connected", ACCENT)
    } else if player.ready {
        ("Ready", MUTED)
    } else {
        ("Starting UxPlay", WARNING)
    };
    let [label, status_area] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(status.width() as u16),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("AirPlay", Style::new().fg(ACCENT).bold()),
            Span::styled(format!("  /  {name}"), Style::new().fg(TEXT)),
        ])),
        label,
    );
    frame.render_widget(
        Paragraph::new(status)
            .style(Style::new().fg(color))
            .right_aligned(),
        status_area,
    );
}

fn card(title: impl Into<Line<'static>>) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(BORDER))
        .title(title.into().style(Style::new().fg(MUTED)))
        .style(Style::new().bg(SURFACE).fg(TEXT))
}

fn draw_track(
    frame: &mut Frame,
    player: &Player,
    area: Rect,
    wide: bool,
    native_graphics: bool,
) -> Option<Rect> {
    let title = match player.visualizer {
        Some(Visualizer::Ribbons) => " Now Playing · Ribbons ",
        Some(Visualizer::Spectrum) => " Now Playing · Spectrum ",
        None => " Now Playing ",
    };
    let shell = card(title);
    let content = shell.inner(area);
    frame.render_widget(shell, area);
    if content.is_empty() {
        return None;
    }

    let [cover_area, details] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(3)]).areas(content);
    let details = if wide || player.visualizer.is_some() {
        details
    } else {
        content
    };
    let lines = if let Some(track) = &player.track {
        vec![
            Line::styled(track.title.as_str(), Style::new().fg(TEXT).bold()),
            Line::styled(track.artist.as_str(), Style::new().fg(ACCENT)),
            Line::styled(track.album.as_str(), Style::new().fg(MUTED)),
        ]
    } else {
        let message = if player.connected {
            "Waiting for track details"
        } else {
            "Select receiver in Apple Music"
        };
        vec![Line::styled(message, Style::new().fg(MUTED))]
    };
    frame.render_widget(Paragraph::new(lines).centered(), details);

    if let Some(visualizer) = player.visualizer {
        draw_visualizer(frame, cover_area, visualizer, player.spectrum.as_ref());
        return None;
    }
    if !wide || cover_area.height < 3 {
        return None;
    }
    if let Some(image) = &player.artwork {
        let cover = fit_cover(cover_area, image);
        if native_graphics {
            return Some(cover);
        }
        draw_half_blocks(frame, image, cover);
    } else if player.track.is_some() {
        let line = Rect::new(
            cover_area.x,
            cover_area.y + cover_area.height / 2,
            cover_area.width,
            1,
        );
        frame.render_widget(
            Paragraph::new("No artwork")
                .style(Style::new().fg(DIM))
                .centered(),
            line,
        );
    }
    None
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
        area.y + (area.height - chart_height) / 2,
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
        area.y + (area.height - height) / 2,
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
            .draw(|frame| screen = draw(frame, &player, "Test", true, now))
            .unwrap();
        let cover = screen.cover.unwrap();
        assert!(
            cover.width >= minimum,
            "{columns}×{rows} terminal used a {cover:?} cover"
        );
        assert!(cover.width.abs_diff(2 * cover.height) <= 1);

        terminal
            .draw(|frame| {
                draw(frame, &player, "Test", false, now);
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

fn draw_buttons(frame: &mut Frame, player: &Player, area: Rect, columns: u16) -> [Rect; 4] {
    let discord = format!("Discord · {}", player.discord.label());
    let labels = [
        "p  Previous",
        "Space  Play/Pause",
        "n  Next",
        discord.as_str(),
    ];
    std::array::from_fn(|index| {
        let row = index as u16 / columns;
        let column = index as u16 % columns;
        if row >= area.height {
            return Rect::ZERO;
        }
        let start = area.width * column / columns;
        let end = area.width * (column + 1) / columns;
        let button = Rect::new(area.x + start, area.y + row, end - start, 1);
        let color = if index < 3 {
            if player.transport { TEXT } else { DIM }
        } else {
            match player.discord {
                PresenceStatus::Off => MUTED,
                PresenceStatus::Sharing => ACCENT,
                PresenceStatus::Unavailable => WARNING,
            }
        };
        frame.render_widget(
            Paragraph::new(labels[index])
                .centered()
                .style(Style::new().fg(color).bg(SURFACE)),
            button,
        );
        button
    })
}
