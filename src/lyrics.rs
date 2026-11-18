use crate::{receiver::clean_text, state::Track};
use anyhow::Result;
use reqwest::{
    StatusCode,
    blocking::{Client, Response},
};
use serde::{Deserialize, de::DeserializeOwned};
use std::{
    collections::HashMap,
    io::Read,
    ops::Range,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};
use unicode_segmentation::UnicodeSegmentation;

const LRCLIB: &str = "https://lrclib.net";
const MAX_RESPONSE_BYTES: u64 = 512 * 1024;
const WORD_LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
const DURATION_TOLERANCE: f64 = 2.0;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct Key {
    pub track: Track,
    pub duration: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub generation: u64,
    pub key: Key,
}

#[derive(Clone)]
pub enum Lyrics {
    Found {
        plain: Vec<String>,
        synced: Vec<SyncedLine>,
    },
    Instrumental,
    Missing,
    Failed,
}

#[derive(Clone)]
pub struct SyncedLine {
    pub start_ms: u64,
    pub end_ms: Option<u64>,
    pub text: String,
    words: Vec<WordTiming>,
}

#[derive(Clone)]
struct WordTiming {
    start_ms: u64,
    end_ms: Option<u64>,
    range: Range<usize>,
}

impl SyncedLine {
    pub fn has_word_timing(&self) -> bool {
        !self.words.is_empty()
    }

    pub fn highlight(&self, position_ms: u64) -> Option<Range<usize>> {
        if self.words.is_empty() {
            return Some(0..self.text.len());
        }
        let started = self
            .words
            .partition_point(|word| word.start_ms <= position_ms);
        self.words[..started]
            .iter()
            .enumerate()
            .rev()
            .find(|(index, word)| {
                word.end_ms
                    .map_or(index + 1 == started, |end| position_ms < end)
            })
            .map(|(_, word)| word.range.clone())
    }

    pub fn lit_ranges(&self, position_ms: u64, next_line_ms: Option<u64>) -> Vec<Range<usize>> {
        if self.words.is_empty() {
            return self.highlight(position_ms).into_iter().collect();
        }
        let mut ranges = Vec::new();
        for word in self
            .words
            .iter()
            .take_while(|word| word.start_ms <= position_ms)
        {
            let segment = &self.text[word.range.clone()];
            let text = segment.trim();
            if text.is_empty() {
                continue;
            }
            let byte_start = word.range.start + (segment.len() - segment.trim_start().len());
            let end_ms = word
                .end_ms
                .or_else(|| {
                    let next = self
                        .words
                        .partition_point(|next| next.start_ms <= word.start_ms);
                    self.words
                        .get(next)
                        .map(|next| next.start_ms)
                        .or(self.end_ms)
                        .or(next_line_ms)
                        .filter(|end| *end > word.start_ms)
                })
                .unwrap_or_else(|| word.start_ms.saturating_add(500));

            let lit_bytes = if position_ms >= end_ms {
                text.len()
            } else {
                let mut graphemes = text.grapheme_indices(true);
                let elapsed_ms = u128::from(position_ms - word.start_ms);
                let duration_ms = u128::from(end_ms - word.start_ms);
                let lit =
                    (elapsed_ms * graphemes.clone().count() as u128 / duration_ms) as usize + 1;
                graphemes.nth(lit).map_or(text.len(), |(byte, _)| byte)
            };
            ranges.push(byte_start..byte_start + lit_bytes);
        }
        ranges.sort_unstable_by_key(|range| range.start);
        ranges
    }
}

impl Lyrics {
    fn has_word_timing(&self) -> bool {
        matches!(self, Self::Found { synced, .. } if synced.iter().any(SyncedLine::has_word_timing))
    }

    pub fn from_text(plain: Option<&str>, synced: Option<&str>) -> Self {
        Self::from_lines(plain, synced.map(parse_lrc).unwrap_or_default())
    }

    fn from_lines(plain: Option<&str>, synced: Vec<SyncedLine>) -> Self {
        let mut plain: Vec<_> = plain.unwrap_or_default().lines().map(clean_text).collect();
        if plain.iter().all(String::is_empty) {
            plain = synced.iter().map(|line| line.text.clone()).collect();
        }
        if plain.iter().all(String::is_empty) {
            Self::Missing
        } else {
            Self::Found { plain, synced }
        }
    }
}

fn parse_lrc(text: &str) -> Vec<SyncedLine> {
    let mut result = Vec::new();
    let mut offset = 0_i64;
    for line in text.lines().map(str::trim) {
        if let Some(value) = line
            .strip_prefix("[offset:")
            .and_then(|s| s.strip_suffix(']'))
        {
            offset = value.trim().parse().unwrap_or_default();
            continue;
        }

        let mut remaining = line;
        let mut timestamps = Vec::new();
        while let Some((tag, tail)) = remaining.strip_prefix('[').and_then(|s| s.split_once(']')) {
            let Some(ms) = timestamp(tag) else { break };
            timestamps.push(ms);
            remaining = tail;
        }

        let text = clean_text(remaining);
        result.extend(timestamps.into_iter().map(|start_ms| SyncedLine {
            start_ms,
            end_ms: None,
            text: text.clone(),
            words: Vec::new(),
        }));
    }
    for line in &mut result {
        line.start_ms = line.start_ms.saturating_add_signed(offset);
    }
    result.sort_by_key(|line| line.start_ms);
    result
}

fn timestamp(tag: &str) -> Option<u64> {
    let (minutes, seconds) = tag.split_once(':')?;
    let (seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let seconds = seconds.parse::<u64>().ok()?;
    if seconds >= 60 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let fractional_ms = fraction
        .bytes()
        .take(3)
        .zip([100, 10, 1])
        .map(|(digit, scale)| u64::from(digit - b'0') * scale)
        .sum::<u64>();
    minutes
        .parse::<u64>()
        .ok()?
        .checked_mul(60_000)?
        .checked_add(seconds * 1000 + fractional_ms)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    duration: Option<f64>,
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
    lyricsfile: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TrackSummary {
    id: u64,
    track_name: String,
    artist_name: String,
    album_name: String,
    duration: f64,
    has_word_sync: bool,
}

#[derive(Deserialize)]
struct Lyricsfile {
    version: String,
    #[serde(default)]
    metadata: LyricsfileMetadata,
    #[serde(default)]
    lines: Vec<LyricsfileLine>,
    plain: Option<String>,
}

#[derive(Default, Deserialize)]
struct LyricsfileMetadata {
    #[serde(default)]
    offset_ms: i64,
}

#[derive(Deserialize)]
struct LyricsfileLine {
    text: String,
    start_ms: u64,
    end_ms: Option<u64>,
    #[serde(default)]
    words: Vec<LyricsfileWord>,
}

#[derive(Deserialize)]
struct LyricsfileWord {
    text: String,
    start_ms: u64,
    end_ms: Option<u64>,
}

impl LyricsfileLine {
    fn into_synced(self, offset: i64) -> SyncedLine {
        let mut text = String::new();
        let mut words = Vec::new();
        for word in self.words {
            let start = text.len();
            text.extend(
                word.text
                    .chars()
                    .filter(|character| !character.is_control()),
            );
            if start < text.len() {
                words.push(WordTiming {
                    start_ms: word.start_ms.saturating_add_signed(offset),
                    end_ms: word
                        .end_ms
                        .filter(|end| *end >= word.start_ms)
                        .map(|end| end.saturating_add_signed(offset)),
                    range: start..text.len(),
                });
            }
        }
        words.sort_by_key(|word| word.start_ms);
        if text.trim().is_empty() {
            text = clean_text(&self.text);
            words.clear();
        }
        SyncedLine {
            start_ms: self.start_ms.saturating_add_signed(offset),
            end_ms: self
                .end_ms
                .filter(|end| *end >= self.start_ms)
                .map(|end| end.saturating_add_signed(offset)),
            text,
            words,
        }
    }
}

impl From<Record> for Lyrics {
    fn from(record: Record) -> Self {
        if record.instrumental {
            return Self::Instrumental;
        }
        let plain = record.plain_lyrics.as_deref();
        let lrc = record.synced_lyrics.as_deref();
        let lyricsfile = record
            .lyricsfile
            .as_deref()
            .and_then(|contents| yaml_serde::from_str::<Lyricsfile>(contents).ok())
            .filter(|file| file.version == "1.0");
        if let Some(file) = lyricsfile {
            let plain = plain
                .filter(|text| !text.trim().is_empty())
                .or(file.plain.as_deref());
            if file.lines.is_empty() {
                return Self::from_text(plain, lrc);
            }
            let mut synced: Vec<_> = file
                .lines
                .into_iter()
                .map(|line| line.into_synced(file.metadata.offset_ms))
                .collect();
            if lrc.is_some_and(|text| !text.trim().is_empty())
                && !synced.iter().any(SyncedLine::has_word_timing)
            {
                return Self::from_text(plain, lrc);
            }
            synced.sort_by_key(|line| line.start_ms);
            return match Self::from_lines(plain, synced) {
                Self::Missing => Self::from_text(plain, lrc),
                lyrics => lyrics,
            };
        }
        Self::from_text(plain, lrc)
    }
}

fn lookup(client: &Client, base: &str, key: &Key) -> Result<(Lyrics, Option<f64>)> {
    let mut query = vec![
        ("track_name", key.track.title.as_str()),
        ("artist_name", key.track.artist.as_str()),
    ];
    if !key.track.album.is_empty() {
        query.push(("album_name", &key.track.album));
    }
    let duration = key.duration.map(|seconds| seconds.to_string());
    if let Some(seconds) = &duration {
        query.push(("duration", seconds));
    }

    let response = client.get(format!("{base}/api/get")).query(&query).send()?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok((Lyrics::Missing, None));
    }
    let record: Record = read_json(response)?;
    let duration = record.duration;
    Ok((record.into(), duration))
}

fn lookup_word_timing(
    client: &Client,
    base: &str,
    key: &Key,
    matched_duration: Option<f64>,
) -> Option<Lyrics> {
    let duration = key.duration.map(f64::from).or(matched_duration)?;
    let track = &key.track;
    let query = [
        ("track_name", track.title.as_str()),
        ("artist_name", track.artist.as_str()),
    ];
    let response = client
        .get(format!("{base}/api/v2/search"))
        .query(&query)
        .timeout(WORD_LOOKUP_TIMEOUT)
        .send()
        .ok()?;
    let mut candidates: Vec<TrackSummary> = read_json(response).ok()?;
    let title = match_text(&track.title);
    let artist = match_text(&track.artist);
    let album = match_text(&track.album);
    candidates.retain(|entry| {
        entry.has_word_sync
            && (entry.duration - duration).abs() <= DURATION_TOLERANCE
            && match_text(&entry.track_name) == title
            && match_text(&entry.artist_name) == artist
    });
    let reference_duration = matched_duration.unwrap_or(duration);
    candidates.sort_by(|first, second| {
        let album_match =
            |entry: &TrackSummary| !album.is_empty() && match_text(&entry.album_name) == album;
        album_match(second).cmp(&album_match(first)).then_with(|| {
            (first.duration - reference_duration)
                .abs()
                .total_cmp(&(second.duration - reference_duration).abs())
        })
    });

    candidates.into_iter().take(2).find_map(|entry| {
        let response = client
            .get(format!("{base}/api/get/{}", entry.id))
            .timeout(WORD_LOOKUP_TIMEOUT)
            .send()
            .ok()?;
        let lyrics = Lyrics::from(read_json::<Record>(response).ok()?);
        lyrics.has_word_timing().then_some(lyrics)
    })
}

fn match_text(value: &str) -> String {
    value
        .trim()
        .chars()
        .map(|character| match character {
            '‘' | '’' => '\'',
            '‐' | '‑' | '–' | '—' => '-',
            other => other,
        })
        .flat_map(char::to_lowercase)
        .collect()
}

fn read_json<T: DeserializeOwned>(response: Response) -> Result<T> {
    let mut body = Vec::new();
    response
        .error_for_status()?
        .take(MAX_RESPONSE_BYTES + 1)
        .read_to_end(&mut body)?;
    anyhow::ensure!(
        body.len() as u64 <= MAX_RESPONSE_BYTES,
        "LRCLIB response too large"
    );
    Ok(serde_json::from_slice(&body)?)
}

pub struct Worker {
    send: Sender<Request>,
    pub results: Receiver<(Request, Lyrics)>,
}

impl Worker {
    pub fn spawn() -> Result<Self> {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(6))
            .user_agent(concat!(
                "t-ui-uxplay/",
                env!("CARGO_PKG_VERSION"),
                " (https://github.com/kiensony/t-ui-uxplay)"
            ))
            .build()?;
        Self::start(client, LRCLIB)
    }

    fn start(client: Client, base: &'static str) -> Result<Self> {
        let (send, requests) = mpsc::channel::<Request>();
        let (results, inbox) = mpsc::channel();
        thread::Builder::new()
            .name("lrclib".into())
            .spawn(move || {
                let mut cache = HashMap::<Key, Lyrics>::new();
                while let Ok(first) = requests.recv() {
                    let request = requests.try_iter().last().unwrap_or(first);
                    if let Some(cached) = cache.get(&request.key) {
                        if results.send((request, cached.clone())).is_err() {
                            break;
                        }
                        continue;
                    }

                    let (mut lyrics, duration) =
                        lookup(&client, base, &request.key).unwrap_or((Lyrics::Failed, None));
                    if results.send((request.clone(), lyrics.clone())).is_err() {
                        break;
                    }
                    if matches!(lyrics, Lyrics::Found { .. })
                        && !lyrics.has_word_timing()
                        && let Some(upgrade) =
                            lookup_word_timing(&client, base, &request.key, duration)
                    {
                        lyrics = upgrade;
                        if results.send((request.clone(), lyrics.clone())).is_err() {
                            break;
                        }
                    }
                    if !matches!(lyrics, Lyrics::Failed) {
                        cache.insert(request.key, lyrics);
                    }
                }
            })?;
        Ok(Self {
            send,
            results: inbox,
        })
    }

    pub fn request(&self, request: Request) {
        let _ = self.send.send(request);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Player;
    use ratatui::{Terminal, backend::TestBackend, buffer::Cell, style::Modifier};
    use std::time::{Instant, UNIX_EPOCH};

    fn lyric_cells<'a>(terminal: &'a Terminal<TestBackend>, lyric: &str) -> &'a [Cell] {
        terminal
            .backend()
            .buffer()
            .content
            .windows(lyric.len())
            .find(|cells| {
                cells
                    .iter()
                    .zip(lyric.bytes())
                    .all(|(cell, byte)| cell.symbol().as_bytes() == [byte])
            })
            .unwrap()
    }

    fn bold(cell: &Cell) -> bool {
        cell.modifier.contains(Modifier::BOLD)
    }

    #[test]
    fn short_final_word_lights_and_next_line_takes_over_on_its_first_word() {
        let record = Record {
            duration: None,
            instrumental: false,
            plain_lyrics: None,
            synced_lyrics: None,
            lyricsfile: Some(
                r#"
version: '1.0'
lines:
  - text: Go X
    start_ms: 10000
    end_ms: 10820
    words:
      - {text: 'Go ', start_ms: 10000, end_ms: 10795}
      - {text: X, start_ms: 10795, end_ms: 10820}
  - text: Tune in
    start_ms: 10870
    words:
      - {text: 'Tune ', start_ms: 10920, end_ms: 11320}
      - {text: in, start_ms: 11320, end_ms: 11720}
"#
                .into(),
            ),
        };
        let now = Instant::now();
        let mut player = Player::default();
        player.lyrics = Some(record.into());
        player.progress(10, 60, now, UNIX_EPOCH + Duration::from_secs(100));
        let mut terminal = Terminal::new(TestBackend::new(100, 26)).unwrap();

        for (offset_ms, final_word_lit, next_word_lit) in [
            (294, false, false),
            (300, true, false),
            (350, true, false),
            (400, true, false),
            (420, false, true),
        ] {
            let at = now + Duration::from_millis(offset_ms);
            terminal
                .draw(|frame| {
                    crate::ui::draw(frame, &player, "Test", false, at);
                })
                .unwrap();
            assert_eq!(
                (
                    bold(&lyric_cells(&terminal, "Go X")[3]),
                    bold(&lyric_cells(&terminal, "Tune in")[0]),
                ),
                (final_word_lit, next_word_lit),
                "lyric highlights at {offset_ms}ms",
            );
        }
    }

    #[test]
    fn lrclib_word_timings_follow_subseconds_stall_and_backward_seeks() {
        let record: Record = serde_json::from_value(serde_json::json!({
            "instrumental": false,
            "plainLyrics": "Cold rain falls",
            "syncedLyrics": "[00:10.00]Cold rain falls",
            "lyricsfile": r#"
version: '1.0'
metadata: {title: Test Song, artist: Test Artist}
lines:
  - text: Cold rain falls
    start_ms: 10500
    end_ms: 12500
    words:
      - {text: 'Cold ', start_ms: 10500, end_ms: 10900}
      - {text: 'rain ', start_ms: 10900, end_ms: 11300}
      - {text: falls, start_ms: 11400}
  - {text: Next line, start_ms: 13000}
"#,
        }))
        .unwrap();
        let mut player = Player::default();
        let now = Instant::now();
        player.lyrics = Some(record.into());
        assert_eq!(player.lyric_view(now).lines, ["Cold rain falls"]);
        player.progress(10, 60, now, UNIX_EPOCH + Duration::from_secs(100));
        let assert_highlight = |player: &Player, offset_ms, expected| {
            let view = player.lyric_view(now + Duration::from_millis(offset_ms));
            let text = view.active.map(|line| view.lines[line]);
            assert_eq!(
                text.zip(view.highlight).map(|(line, range)| &line[range]),
                expected,
                "highlight at {offset_ms}ms"
            );
        };

        assert_highlight(&player, 150, Some("Cold "));
        assert_highlight(&player, 500, Some("rain "));
        let mut terminal = Terminal::new(TestBackend::new(100, 26)).unwrap();
        terminal
            .draw(|frame| {
                crate::ui::draw(
                    frame,
                    &player,
                    "Test",
                    false,
                    now + Duration::from_millis(475),
                );
            })
            .unwrap();
        let lyric_line = lyric_cells(&terminal, "Cold rain falls");
        assert!(lyric_line[..4].iter().all(bold));
        assert!(bold(&lyric_line[5]));
        assert!(lyric_line[6..].iter().all(|cell| !bold(cell)));
        assert_highlight(&player, 850, None);
        assert_highlight(&player, 950, Some("falls"));
        assert_highlight(&player, 6000, Some("falls"));

        player.progress(
            12,
            60,
            now + Duration::from_millis(6100),
            UNIX_EPOCH + Duration::from_millis(106100),
        );
        assert_highlight(&player, 6300, None);
        let sample = now + Duration::from_secs(7);
        player.progress(13, 60, sample, UNIX_EPOCH + Duration::from_secs(107));
        assert_highlight(&player, 7000, Some("Next line"));
        player.progress(
            10,
            60,
            now + Duration::from_secs(8),
            UNIX_EPOCH + Duration::from_secs(108),
        );
        assert_highlight(&player, 8100, Some("Cold "));
    }

    #[test]
    fn lyricsfiles_without_usable_word_timings_keep_line_synced_lyrics() {
        for lyricsfile in [
            "version: '1.0'\nlines: [unfinished",
            "version: '1.0'\nlines: [{text: Another line, start_ms: 13000}]",
        ] {
            let record: Record = serde_json::from_value(serde_json::json!({
                "instrumental": false,
                "plainLyrics": "Plain line",
                "syncedLyrics": "[00:12.00]Synced line",
                "lyricsfile": lyricsfile,
            }))
            .unwrap();
            let mut player = Player::default();
            let now = Instant::now();
            player.lyrics = Some(record.into());
            player.progress(12, 60, now, UNIX_EPOCH + Duration::from_secs(100));
            let view = player.lyric_view(now);
            assert_eq!(view.lines, ["Synced line"]);
            assert_eq!(view.highlight, Some(0..11));
        }
    }
    use std::{
        io::{BufRead, BufReader, Write},
        os::unix::net::UnixListener,
    };

    #[test]
    fn upgrades_line_lyrics_from_a_compatible_release_and_caches_the_upgrade() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("lrclib.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            respond(
                &listener,
                "/api/get?track_name=Test+Song&artist_name=Test+Artist&album_name=Studio+Album&duration=192",
                r#"{"id":7,"duration":192,"instrumental":false,"syncedLyrics":"[00:03.25]Slow rain"}"#,
            );
            respond(
                &listener,
                "/api/v2/search?track_name=Test+Song&artist_name=Test+Artist",
                r#"[
                  {"id":7,"trackName":"Test Song","artistName":"Test Artist","albumName":"Studio Album","duration":192,"hasWordSync":false},
                  {"id":8,"trackName":"Test Song","artistName":"Other Artist","albumName":"Studio Album","duration":192,"hasWordSync":true},
                  {"id":9,"trackName":"Test Song (Live)","artistName":"Test Artist","albumName":"Studio Album","duration":192,"hasWordSync":true},
                  {"id":10,"trackName":"Test Song","artistName":"Test Artist","albumName":"Studio Album","duration":202,"hasWordSync":true},
                  {"id":11,"trackName":"test song","artistName":"Test Artist","albumName":"Retrospective","duration":192,"hasWordSync":true}
                ]"#,
            );
            let timed = serde_json::json!({
                "id": 11,
                "instrumental": false,
                "lyricsfile": r#"
version: '1.0'
lines:
  - text: Slow rain
    start_ms: 3250
    words:
      - {text: 'Slow ', start_ms: 3250}
      - {text: rain, start_ms: 3700}
"#,
            });
            respond(&listener, "/api/get/11", &timed.to_string());
        });
        let client = Client::builder()
            .unix_socket(socket)
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let worker = Worker::start(client, "http://mock.lrclib").unwrap();
        let key = Key {
            track: Track {
                title: "Test Song".into(),
                artist: "Test Artist".into(),
                album: "Studio Album".into(),
            },
            duration: Some(192),
        };
        worker.request(Request {
            generation: 1,
            key: key.clone(),
        });
        let (request, original) = worker.results.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(request.generation, 1);
        assert!(!original.has_word_timing());
        let (request, improved) = worker.results.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(request.generation, 1);
        assert!(improved.has_word_timing());
        worker.request(Request { generation: 2, key });
        let (request, cached) = worker.results.recv_timeout(Duration::from_secs(3)).unwrap();
        assert_eq!(request.generation, 2);
        assert!(cached.has_word_timing());
        server.join().unwrap();
    }

    fn respond(listener: &UnixListener, path: &str, payload: &str) {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut stream = BufReader::new(stream);
        let mut line = String::new();
        stream.read_line(&mut line).unwrap();
        assert_eq!(line, format!("GET {path} HTTP/1.1\r\n"));
        loop {
            line.clear();
            if stream.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                break;
            }
        }
        write!(
            stream.get_mut(),
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        )
        .unwrap();
    }

    #[test]
    fn lrclib_get_uses_the_complete_track_signature_and_caches_it() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("lrclib.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = thread::spawn(move || {
            let payload = r#"{"instrumental":false,"plainLyrics":"First\nSecond","syncedLyrics":"[00:01.25]First\n[00:02.50]Second"}"#;
            respond(
                &listener,
                "/api/get?track_name=One+%26+Two&artist_name=An+Artist&album_name=An+Album&duration=180",
                payload,
            );
        });
        let client = Client::builder()
            .unix_socket(socket)
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let worker = Worker::start(client, "http://mock.lrclib").unwrap();
        let key = Key {
            track: Track {
                title: "One & Two".into(),
                artist: "An Artist".into(),
                album: "An Album".into(),
            },
            duration: Some(180),
        };
        for generation in [1, 2] {
            worker.request(Request {
                generation,
                key: key.clone(),
            });
            let (request, result) = worker.results.recv_timeout(Duration::from_secs(3)).unwrap();
            assert_eq!(request.generation, generation);
            let Lyrics::Found { plain, synced } = result else {
                panic!("Expected lyrics")
            };
            assert_eq!(plain, ["First", "Second"]);
            assert_eq!(synced[1].start_ms, 2500);
        }
        server.join().unwrap();
    }
}
