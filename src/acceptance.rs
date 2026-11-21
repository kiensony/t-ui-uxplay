use crate::{dacp, discord, lyrics, receiver, state, ui};
use std::time::{Duration, Instant, UNIX_EPOCH};

#[test]
fn now_playing_cycles_through_cover_ribbons_and_spectrum() {
    use crossterm::event::KeyCode;
    use ratatui::{Terminal, backend::TestBackend};

    assert_eq!(
        ui::key_action(KeyCode::Char('v')),
        Some(ui::Action::CycleVisualizer)
    );
    let mut player = state::Player::default();
    player.artwork = Some(image::DynamicImage::new_rgb8(16, 16));
    let mut spectrum = crate::spectrum::Spectrum::default();
    spectrum.levels[10] = 0.65;
    spectrum.peaks[10] = 0.85;
    player.spectrum = Some(spectrum);

    let now = Instant::now();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let mut screen = ui::Screen::default();
    terminal
        .draw(|frame| screen = ui::draw(frame, &player, "Test", true, now))
        .unwrap();
    assert!(screen.cover.is_some());

    for (width, height) in [(100, 30), (48, 25)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        player.cycle_visualizer();
        terminal
            .draw(|frame| screen = ui::draw(frame, &player, "Test", true, now))
            .unwrap();
        assert!(screen.cover.is_none());
        let ribbons = terminal.backend().buffer().content.clone();
        assert!(ribbons.iter().any(|cell| {
            cell.symbol()
                .starts_with(|dot| ('\u{2801}'..='\u{28ff}').contains(&dot))
        }));
        spectrum.phase += 0.8;
        player.spectrum = Some(spectrum);
        terminal
            .draw(|frame| screen = ui::draw(frame, &player, "Test", true, now))
            .unwrap();
        assert_ne!(terminal.backend().buffer().content, ribbons);

        player.cycle_visualizer();
        terminal
            .draw(|frame| screen = ui::draw(frame, &player, "Test", true, now))
            .unwrap();
        assert!(screen.cover.is_none());
        let cells = &terminal.backend().buffer().content;
        assert!(cells.iter().any(|cell| cell.symbol() == "▄"));
        assert!(cells.iter().any(|cell| cell.symbol() == "━"));

        player.cycle_visualizer();
        terminal
            .draw(|frame| screen = ui::draw(frame, &player, "Test", true, now))
            .unwrap();
        assert_eq!(screen.cover.is_some(), width == 100);
        assert_eq!(player.visualizer, None);
    }
}

#[test]
fn reads_complete_stock_metadata_and_dacp_exports() {
    let export = include_bytes!("../tests/fixtures/uxplay-metadata");
    assert!(receiver::metadata(&export[..export.len() - 1]).is_none());
    let track = receiver::metadata(export).unwrap().unwrap();
    assert_eq!(track.title, "Blue in Green");
    assert_eq!(track.artist, "Miles Davis");
    assert_eq!(track.album, "Kind of Blue");
    assert!(receiver::metadata(b"no data\n\0").unwrap().is_none());

    assert!(dacp::Credentials::parse(b"A12BCD\n1234").is_none());
    assert!(dacp::Credentials::parse(b"A12BCD\n1234\n").is_some());
}

#[test]
fn artwork_is_optional_and_incomplete_files_are_not_decoded() {
    let art = include_bytes!("../tests/fixtures/cover.png");
    assert!(receiver::decode_cover(&art[..art.len() - 8]).is_none());
    assert!(receiver::decode_cover(art).unwrap().is_some());
    assert!(
        receiver::decode_cover(include_bytes!("../tests/fixtures/empty.png"))
            .unwrap()
            .is_none()
    );
    let state = state::Player::default();
    assert!(state.artwork.is_none());
}

#[test]
fn parses_carriage_return_progress_and_connection_events() {
    use receiver::LogEvent::*;
    let log = b"Open connections: 1\naudio progress (min:sec):   0:12; remaining:   3:25; track length 3:37\rOpen connections: 0\n";
    let (send, recv) = std::sync::mpsc::channel();
    receiver::read_log(&log[..], send).unwrap();
    let events: Vec<_> = recv.into_iter().map(|log| log.event).collect();
    assert_eq!(events, [Connected, Progress(12, 217), Disconnected]);
    assert_eq!(receiver::parse_log("TEARDOWN request,  96=0, 110=1"), None);
    assert_eq!(
        receiver::parse_log("TEARDOWN request,  96=1, 110=0"),
        Some(Disconnected)
    );
}

#[test]
fn late_lyrics_cannot_replace_the_active_track() {
    let mut player = state::Player::default();
    let now = Instant::now();
    player.set_track(Some(track("One")), now);
    let old = player.lyrics_request(now + Duration::from_secs(2)).unwrap();
    player.set_track(Some(track("Two")), now);
    let current = player.lyrics_request(now + Duration::from_secs(2)).unwrap();
    player.set_lyrics(&current, lyrics::Lyrics::Instrumental);
    player.set_lyrics(&old, lyrics::Lyrics::Missing);
    assert!(matches!(player.lyrics, Some(lyrics::Lyrics::Instrumental)));
    player.disconnect(now);
    player.set_lyrics(&current, lyrics::Lyrics::Instrumental);
    assert!(player.lyrics.is_none());
}

#[test]
fn progress_stalls_and_backward_seeks_select_the_correct_line() {
    let now = Instant::now();
    let mut player = state::Player::default();
    player.set_track(Some(track("One")), now);
    player.progress(12, 100, now, UNIX_EPOCH + Duration::from_secs(100));
    let request = player.lyrics_request(now + Duration::from_secs(2)).unwrap();
    assert_eq!(request.key.duration, Some(100));
    player.set_lyrics(
        &request,
        lyrics::Lyrics::from_text(
            None,
            Some("[00:02.05]First\n[00:10.00]Second\n[00:13.5]Third"),
        ),
    );
    assert_eq!(player.lyric_view(now).active, Some(1));
    assert_eq!(
        player.lyric_view(now + Duration::from_secs(30)).active,
        Some(1)
    );
    player.progress(
        4,
        100,
        now + Duration::from_secs(31),
        UNIX_EPOCH + Duration::from_secs(131),
    );
    assert_eq!(
        player.lyric_view(now + Duration::from_secs(31)).active,
        Some(0)
    );
    player.set_track(Some(track("Two")), now);
    assert!(player.timing.is_none());
}

#[test]
fn line_highlighting_compensates_for_rounded_progress_and_advances_between_reports() {
    let now = Instant::now();
    let mut player = state::Player::default();
    player.lyrics = Some(lyrics::Lyrics::from_text(
        None,
        Some("[00:12.35]First line\n[00:12.82]Second line\n[00:13.60]Third line"),
    ));
    player.progress(12, 100, now, UNIX_EPOCH + Duration::from_secs(100));

    assert_eq!(player.lyric_view(now).active, Some(0));
    let nudge_time = now + Duration::from_millis(100);
    for (key, expected) in [(']', 1), ('[', 0)] {
        let Some(ui::Action::AdjustLyricSync(delta_ms)) =
            ui::key_action(crossterm::event::KeyCode::Char(key))
        else {
            panic!("Missing lyric sync shortcut");
        };
        player.adjust_lyric_sync(delta_ms);
        assert_eq!(player.lyric_view(nudge_time).active, Some(expected));
        assert_eq!(player.timing.unwrap().position, 12);
    }
    let later = now + Duration::from_millis(400);
    assert_eq!(player.lyric_view(later).active, Some(1));
    assert_eq!(
        player.lyric_view(now + Duration::from_secs(30)).active,
        Some(1)
    );

    let next_report = Duration::from_millis(1100);
    player.progress(
        13,
        100,
        now + next_report,
        UNIX_EPOCH + Duration::from_secs(100) + next_report,
    );
    assert_eq!(player.lyric_view(now + next_report).active, Some(2));
}

#[test]
fn unavailable_transport_cannot_be_clicked_in_compact_layout() {
    use crossterm::event::KeyCode;
    use ratatui::{Terminal, backend::TestBackend};
    let mut terminal = Terminal::new(TestBackend::new(38, 18)).unwrap();
    let player = state::Player::default();
    let mut screen = None;
    terminal
        .draw(|frame| {
            screen = Some(ui::draw(frame, &player, "Home", false, Instant::now()));
        })
        .unwrap();
    let screen = screen.unwrap();
    let next = screen.buttons[2];
    assert!(next.width > 0);
    assert_eq!(screen.click(next.x, next.y), None);
    assert_eq!(
        ui::key_action(KeyCode::Char('n')),
        Some(ui::Action::Transport(dacp::Command::Next))
    );
    let button = screen.buttons[3];
    assert_eq!(screen.click(button.x, button.y), Some(ui::Action::Discord));
}

#[test]
fn discord_activity_contains_song_artist_and_listening_type() {
    let activity = discord::Listening::new(&track("One"), None, Instant::now()).payload();
    assert_eq!(activity["type"], 2);
    assert_eq!(activity["name"], "One");
    assert_eq!(activity["status_display_type"], 2);
    assert_eq!(activity["details"], "One");
    assert_eq!(activity["state"], "Miles Davis");
}

#[test]
fn discord_progress_reanchors_after_seeks_and_holds_when_playback_stalls() {
    let mut player = state::Player::default();
    let start = Instant::now();
    let epoch_seconds = 1_790_000_000;
    let wall = UNIX_EPOCH + Duration::from_secs(epoch_seconds);
    player.progress(42, 210, start, wall);
    player.set_track(Some(track("One")), start);
    let payload = |player: &state::Player, offset: Duration| {
        discord::Listening::new(
            player.track.as_ref().unwrap(),
            player.timing,
            start + offset,
        )
        .payload()
    };
    let timestamps = |offset_ms: u64, elapsed: u64| {
        let start_ms = epoch_seconds * 1000 + offset_ms - elapsed * 1000;
        serde_json::json!({ "start": start_ms, "end": start_ms + 210_000 })
    };
    let sample = |player: &mut state::Player, offset_ms: u64, position| {
        let offset = Duration::from_millis(offset_ms);
        player.progress(position, 210, start + offset, wall + offset);
        payload(player, offset)
    };

    assert_eq!(
        payload(&player, Duration::ZERO)["timestamps"],
        timestamps(0, 42)
    );
    assert_eq!(
        sample(&mut player, 900, 43)["timestamps"],
        timestamps(0, 42)
    );
    assert_eq!(
        sample(&mut player, 1000, 19)["timestamps"],
        timestamps(1000, 19)
    );
    assert_eq!(
        sample(&mut player, 1200, 80)["timestamps"],
        timestamps(1200, 80)
    );
    assert_eq!(
        sample(&mut player, 3900, 80)["timestamps"],
        timestamps(1200, 80)
    );

    let stalled = payload(&player, Duration::from_millis(4300));
    assert!(stalled.get("timestamps").is_none());
    assert_eq!(stalled["state"], "Miles Davis · 1:20 / 3:30");

    assert_eq!(
        sample(&mut player, 6000, 81)["timestamps"],
        timestamps(6000, 81)
    );
    player.set_track(Some(track("Two")), start + Duration::from_secs(7));
    let changed = payload(&player, Duration::from_secs(7));
    assert_eq!(changed["name"], "Two");
    assert!(changed.get("timestamps").is_none());
}

fn track(title: &str) -> state::Track {
    state::Track {
        title: title.to_owned(),
        artist: "Miles Davis".to_owned(),
        album: "Kind of Blue".to_owned(),
    }
}
