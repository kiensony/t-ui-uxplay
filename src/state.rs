use crate::{
    lyrics::{Key, Lyrics, Request},
    spectrum::Spectrum,
};
use image::DynamicImage;
use std::{
    ops::Range,
    time::{Duration, Instant, SystemTime},
};

const ARTWORK_SETTLE: Duration = Duration::from_millis(350);
const LYRICS_DELAY: Duration = Duration::from_millis(1200);
const RESUME_FOLLOWING: Duration = Duration::from_secs(8);
const PROGRESS_STALL: Duration = Duration::from_secs(3);
const SEEK_TOLERANCE: Duration = Duration::from_millis(1500);
const PROGRESS_INTERVAL_MS: u64 = 1000;

#[derive(Clone, Debug, Default, Hash, PartialEq, Eq)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
}

#[derive(Clone, Copy)]
pub struct Timing {
    pub position: u32,
    pub duration: u32,
    sampled_at: Instant,
    lyric_sample_ms: u64,
    anchor_at: Instant,
    anchor_position: u32,
    pub started_at: Option<SystemTime>,
}

impl Timing {
    pub fn is_advancing(self, now: Instant) -> bool {
        self.position < self.duration && now.duration_since(self.sampled_at) < PROGRESS_STALL
    }

    fn lyric_position_ms(self, now: Instant, offset_ms: i64) -> u64 {
        let elapsed_ms = now
            .duration_since(self.sampled_at)
            .as_millis()
            .min(u128::from(PROGRESS_INTERVAL_MS - 1)) as u64;
        let duration_upper_bound =
            u64::from(self.duration) * PROGRESS_INTERVAL_MS + (PROGRESS_INTERVAL_MS - 1);
        (self.lyric_sample_ms + elapsed_ms)
            .saturating_add_signed(offset_ms)
            .min(duration_upper_bound)
    }

    fn matches_playback(self, position: u32, duration: u32, now: Instant) -> bool {
        let expected_position =
            Duration::from_secs(self.anchor_position.into()) + now.duration_since(self.anchor_at);
        self.duration == duration
            && position >= self.position
            && self.is_advancing(now)
            && expected_position.abs_diff(Duration::from_secs(position.into())) < SEEK_TOLERANCE
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum PresenceStatus {
    #[default]
    Off,
    Sharing,
    Unavailable,
}

impl PresenceStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Sharing => "Sharing",
            Self::Unavailable => "Unavailable",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visualizer {
    Ribbons,
    Spectrum,
}

#[derive(Default)]
pub struct Player {
    pub ready: bool,
    pub connected: bool,
    pub transport: bool,
    pub discord: PresenceStatus,
    pub lyrics_disabled: bool,
    pub track: Option<Track>,
    pub timing: Option<Timing>,
    pub artwork: Option<DynamicImage>,
    pub artwork_revision: u64,
    pub visualizer: Option<Visualizer>,
    pub spectrum: Option<Spectrum>,
    pub lyrics: Option<Lyrics>,
    pub lyric_offset_ms: i64,
    pending_artwork: Option<(DynamicImage, Instant)>,
    generation: u64,
    track_since: Option<Instant>,
    lyrics_requested: Option<Request>,
    scroll: Option<(usize, Instant)>,
}

impl Player {
    pub fn cycle_visualizer(&mut self) {
        self.visualizer = match self.visualizer {
            None => Some(Visualizer::Ribbons),
            Some(Visualizer::Ribbons) => Some(Visualizer::Spectrum),
            Some(Visualizer::Spectrum) => None,
        };
    }

    pub fn set_track(&mut self, track: Option<Track>, now: Instant) {
        if track == self.track {
            return;
        }
        let artwork = if track.is_some() {
            self.pending_artwork.take().map(|(artwork, _)| artwork)
        } else {
            self.pending_artwork = None;
            None
        };
        self.replace_artwork(artwork);
        // A new stream can report progress before it exports its first title.
        if self.track.is_some() || track.is_none() {
            self.timing = None;
        }
        self.track = track;
        self.generation += 1;
        self.track_since = self.track.as_ref().map(|_| now);
        self.lyrics = None;
        self.lyrics_requested = None;
        self.scroll = None;
    }

    pub fn stream_started(&mut self, now: Instant) {
        self.set_track(None, now);
        self.timing = None;
        self.pending_artwork = None;
        self.connected = true;
    }

    pub fn disconnect(&mut self, now: Instant) {
        self.stream_started(now);
        self.connected = false;
        self.transport = false;
    }

    pub fn progress(&mut self, position: u32, duration: u32, now: Instant, wall_time: SystemTime) {
        if self
            .timing
            .is_some_and(|timing| timing.position == position && timing.duration == duration)
        {
            return;
        }
        // UxPlay truncates to whole seconds. Keep the estimate in that second
        // and use its midpoint when starting or re-anchoring the clock.
        let start_ms = u64::from(position) * PROGRESS_INTERVAL_MS;
        let end_ms = start_ms + (PROGRESS_INTERVAL_MS - 1);
        let timing = self
            .timing
            .filter(|previous| previous.matches_playback(position, duration, now))
            .map(|previous| Timing {
                position,
                sampled_at: now,
                lyric_sample_ms: (previous.lyric_sample_ms
                    + now.duration_since(previous.sampled_at).as_millis() as u64)
                    .clamp(start_ms, end_ms),
                ..previous
            })
            .unwrap_or_else(|| Timing {
                position,
                duration,
                sampled_at: now,
                lyric_sample_ms: start_ms + PROGRESS_INTERVAL_MS / 2,
                anchor_at: now,
                anchor_position: position,
                started_at: wall_time.checked_sub(Duration::from_secs(position.into())),
            });
        self.timing = Some(timing);
    }

    pub fn stage_artwork(&mut self, artwork: Option<DynamicImage>, now: Instant) {
        if let Some(artwork) = artwork {
            self.pending_artwork = Some((artwork, now));
        } else {
            self.pending_artwork = None;
            self.replace_artwork(None);
        }
    }

    pub fn settle_artwork(&mut self, now: Instant) {
        if self.track.is_some()
            && self
                .pending_artwork
                .as_ref()
                .is_some_and(|(_, since)| now.duration_since(*since) >= ARTWORK_SETTLE)
        {
            let artwork = self.pending_artwork.take().map(|(artwork, _)| artwork);
            self.replace_artwork(artwork);
        }
    }

    fn replace_artwork(&mut self, artwork: Option<DynamicImage>) {
        if self.artwork.is_some() || artwork.is_some() {
            self.artwork_revision += 1;
        }
        self.artwork = artwork;
    }

    pub fn lyrics_request(&mut self, now: Instant) -> Option<Request> {
        if self.lyrics_disabled || now.duration_since(self.track_since?) < LYRICS_DELAY {
            return None;
        }
        let track = self.track.as_ref()?;
        if track.artist.is_empty() {
            return None;
        }
        let duration = self
            .timing
            .map(|timing| timing.duration)
            .filter(|seconds| (1..=3600).contains(seconds));
        let request = Request {
            generation: self.generation,
            key: Key {
                track: track.clone(),
                duration,
            },
        };
        if self.lyrics_requested.as_ref() == Some(&request) {
            return None;
        }
        self.lyrics_requested = Some(request.clone());
        Some(request)
    }

    pub fn set_lyrics(&mut self, request: &Request, lyrics: Lyrics) {
        if self.lyrics_requested.as_ref() == Some(request) {
            self.lyrics = Some(lyrics);
            self.scroll = None;
        }
    }

    pub fn lyric_view(&self, now: Instant) -> LyricView<'_> {
        if let Some(Lyrics::Found { plain, synced }) = &self.lyrics {
            let timing = self.timing.filter(|_| !synced.is_empty());
            let position = timing.map(|timing| timing.lyric_position_ms(now, self.lyric_offset_ms));
            let (active, highlight, lit) = position.map_or((None, None, Vec::new()), |position| {
                let started = synced.partition_point(|line| line.start_ms <= position);
                let active = (0..started).rev().find(|&index| {
                    synced[index]
                        .end_ms
                        .map_or(index + 1 == started, |end| position < end)
                });
                let lit_line = |index: usize| {
                    let next_line_ms = synced.get(index + 1).map(|line| line.start_ms);
                    synced[index].lit_ranges(position, next_line_ms)
                };
                let lit = active.map_or_else(Vec::new, lit_line);
                // Keep the last sung line visible until the next one has lyrics to light.
                if lit.is_empty()
                    && let Some(previous) = active.unwrap_or(started).checked_sub(1)
                    && previous + 1 < synced.len()
                    && synced[previous].has_word_timing()
                {
                    let previous_lit = lit_line(previous);
                    if !previous_lit.is_empty() {
                        return (Some(previous), None, previous_lit);
                    }
                }
                let highlight = active.and_then(|index| synced[index].highlight(position));
                (active, highlight, lit)
            });
            let lines = if timing.is_some() {
                synced.iter().map(|line| line.text.as_str()).collect()
            } else {
                plain.iter().map(String::as_str).collect()
            };
            let scroll = self
                .scroll
                .filter(|(_, at)| timing.is_none() || now < *at)
                .map(|(top, _)| top);
            return LyricView {
                lines,
                active,
                highlight,
                lit,
                scroll,
                message: None,
                sync_label: timing.map(|_| {
                    if synced.iter().any(|line| line.has_word_timing()) {
                        "Word sync"
                    } else {
                        "Line sync"
                    }
                }),
            };
        }

        let message = match &self.lyrics {
            Some(Lyrics::Instrumental) => "Instrumental track",
            Some(Lyrics::Missing) => "No lyrics found",
            Some(Lyrics::Failed) => "Lyrics lookup failed",
            Some(Lyrics::Found { .. }) => unreachable!(),
            None if self.lyrics_disabled => "Online lyrics disabled",
            None if self.track.is_none() => "Lyrics appear when a track starts",
            None if self
                .track
                .as_ref()
                .is_some_and(|track| track.artist.is_empty()) =>
            {
                "Waiting for artist information"
            }
            None => "Looking up lyrics…",
        };
        LyricView {
            message: Some(message),
            ..LyricView::default()
        }
    }

    pub fn scroll_lyrics(&mut self, top: usize, now: Instant) {
        self.scroll = Some((top, now + RESUME_FOLLOWING));
    }

    pub fn adjust_lyric_sync(&mut self, delta_ms: i64) {
        self.lyric_offset_ms = self.lyric_offset_ms.saturating_add(delta_ms);
    }
}

pub fn format_time(seconds: u32) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[derive(Default)]
pub struct LyricView<'a> {
    pub lines: Vec<&'a str>,
    pub active: Option<usize>,
    pub highlight: Option<Range<usize>>,
    pub lit: Vec<Range<usize>>,
    pub scroll: Option<usize>,
    pub message: Option<&'static str>,
    pub sync_label: Option<&'static str>,
}
