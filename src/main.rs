mod dacp;
mod discord;
mod lyrics;
mod receiver;
mod state;
mod terminal;
mod ui;

#[cfg(test)]
mod acceptance;

use anyhow::{Context, Result, bail};
use clap::Parser;
use crossterm::event::{
    self, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
};
use receiver::{LogEvent, Receiver};
use signal_hook::{
    consts::signal::{SIGHUP, SIGINT, SIGTERM},
    flag,
};
use state::{Player, PresenceStatus};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use terminal::PlayerTerminal;
use ui::Action;

#[derive(Parser)]
#[command(version, about)]
struct Options {
    /// Name shown in the sender's AirPlay audio picker
    #[arg(long, default_value = "t-ui-uxplay", value_parser = receiver_name)]
    name: String,

    /// UxPlay executable or path
    #[arg(long, default_value = "uxplay")]
    uxplay: PathBuf,

    /// Application ID from the Discord Developer Portal
    #[arg(long, value_parser = application_id)]
    discord_app_id: Option<String>,

    /// Keep track details local and skip LRCLIB lookups
    #[arg(long)]
    no_online_lyrics: bool,
}

fn receiver_name(value: &str) -> Result<String, &'static str> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') || value.chars().any(char::is_control) {
        return Err("Use a nonempty AirPlay name without control characters or a leading hyphen");
    }
    Ok(value.to_owned())
}

fn application_id(value: &str) -> Result<String, &'static str> {
    if value.bytes().all(|byte| byte.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|id| id != 0)
    {
        Ok(value.to_owned())
    } else {
        Err("Use the numeric Discord application ID")
    }
}

fn main() -> Result<()> {
    let options = Options::parse();
    let stop = Arc::new(AtomicBool::new(false));
    for signal in [SIGINT, SIGTERM, SIGHUP] {
        flag::register(signal, stop.clone())?;
    }
    let mut terminal = PlayerTerminal::open()?;
    let mut receiver = Receiver::spawn(&options.uxplay, &options.name)?;
    let dacp = dacp::Worker::spawn()?;
    let lyrics = (!options.no_online_lyrics)
        .then(lyrics::Worker::spawn)
        .transpose()?;
    let discord = options
        .discord_app_id
        .map(discord::Worker::spawn)
        .transpose()?;

    let mut session = Session::default();
    session.player.lyrics_disabled = options.no_online_lyrics;
    let mut sharing = Sharing::default();
    while !stop.load(Ordering::Relaxed) {
        session.poll_receiver(&mut receiver, &dacp, Instant::now())?;
        let now = Instant::now();
        sharing.sync(&mut session.player, discord.as_ref(), now);
        if let Some(lyrics) = &lyrics {
            if let Some(request) = session.player.lyrics_request(now) {
                lyrics.request(request);
            }
            for (request, result) in lyrics.results.try_iter() {
                session.player.set_lyrics(&request, result);
            }
        }

        let screen = terminal.draw(&session.player, &options.name, now)?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let action = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => {
                if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                    Some(Action::Quit)
                } else if key.modifiers == KeyModifiers::NONE {
                    ui::key_action(key.code)
                } else {
                    None
                }
            }
            Event::Key(key)
                if key.kind == KeyEventKind::Repeat
                    && key.modifiers == KeyModifiers::NONE
                    && matches!(key.code, KeyCode::Up | KeyCode::Down) =>
            {
                ui::key_action(key.code)
            }
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                screen.click(mouse.column, mouse.row)
            }
            Event::Resize(..) => {
                terminal.resized()?;
                None
            }
            _ => None,
        };
        match action {
            Some(Action::Quit) => break,
            Some(Action::Transport(command)) if session.player.transport => {
                dacp.control(session.transport_revision, command);
            }
            Some(Action::Discord) => {
                sharing.enabled = !sharing.enabled;
                sharing.sync(&mut session.player, discord.as_ref(), Instant::now());
            }
            Some(Action::Scroll(delta)) => {
                let offset = screen
                    .scroll_top
                    .saturating_add_signed(isize::from(delta))
                    .min(screen.scroll_max);
                if screen.scroll_max > 0 {
                    session.player.scroll_lyrics(offset, now);
                }
            }
            Some(Action::AdjustLyricSync(delta_ms)) => session.player.adjust_lyric_sync(delta_ms),
            _ => {}
        }
    }
    Ok(())
}

#[derive(Default)]
struct Session {
    player: Player,
    diagnostic: Option<String>,
    credentials: Option<dacp::Credentials>,
    transport_revision: u64,
}

impl Session {
    fn poll_receiver(
        &mut self,
        receiver: &mut Receiver,
        dacp: &dacp::Worker,
        now: Instant,
    ) -> Result<()> {
        let exit = receiver
            .exit_status()
            .context("Could not check UxPlay status")?;
        let mut progress = None;
        for log in receiver.logs.try_iter() {
            match log.event {
                LogEvent::Ready => self.player.ready = true,
                LogEvent::Connected => self.player.connected = true,
                LogEvent::StreamStarted => {
                    self.player.stream_started(now);
                    progress = None;
                }
                LogEvent::Disconnected => {
                    self.player.disconnect(now);
                    self.set_credentials(dacp, None);
                    progress = None;
                }
                LogEvent::Progress(position, duration) => {
                    progress = Some((position, duration, log.received_at, log.wall_time));
                }
                LogEvent::Diagnostic(message) => self.diagnostic = Some(message),
            }
        }

        if let Some(status) = exit {
            // Drain any diagnostic written just before the process went away.
            while let Ok(log) = receiver.logs.recv_timeout(Duration::from_millis(30)) {
                if let LogEvent::Diagnostic(message) = log.event {
                    self.diagnostic = Some(message);
                }
            }
            self.player.disconnect(now);
            let detail = self
                .diagnostic
                .as_deref()
                .unwrap_or("Check the installed UxPlay binary, GStreamer, and the mDNS service");
            bail!("UxPlay exited ({status}). {detail}");
        }

        if self.player.connected {
            // Export writes can reach disk before their connection log is delivered.
            if let Some(track) = receiver.exports.track() {
                self.player.set_track(track, now);
            }
            if let Some((position, duration, sampled_at, wall_time)) = progress {
                self.player
                    .progress(position, duration, sampled_at, wall_time);
            }
            if let Some(art) = receiver.exports.artwork() {
                self.player.stage_artwork(art, now);
            }
            if let Some(credentials) = receiver.exports.credentials() {
                self.set_credentials(dacp, credentials);
            }
            self.player.settle_artwork(now);
        }

        for (revision, available) in dacp.availability.try_iter() {
            if revision == self.transport_revision {
                self.player.transport = available;
            }
        }
        Ok(())
    }

    fn set_credentials(&mut self, worker: &dacp::Worker, credentials: Option<dacp::Credentials>) {
        if credentials != self.credentials {
            self.credentials = credentials;
            self.player.transport = false;
            self.transport_revision += 1;
            worker.session(self.transport_revision, self.credentials.clone());
        }
    }
}

#[derive(Default)]
struct Sharing {
    enabled: bool,
    submitted: Option<(bool, Option<discord::Listening>)>,
    revision: u64,
}

impl Sharing {
    fn sync(&mut self, player: &mut Player, worker: Option<&discord::Worker>, now: Instant) {
        let activity = if self.enabled {
            player
                .track
                .as_ref()
                .map(|track| discord::Listening::new(track, player.timing, now))
        } else {
            None
        };
        let desired = (self.enabled, activity);
        if self.submitted.as_ref() != Some(&desired) {
            self.revision += 1;
            if let Some(worker) = worker {
                worker.update(self.revision, desired.0, desired.1.clone());
            }
            if !self.enabled {
                player.discord = PresenceStatus::Off;
            } else if worker.is_none() || player.discord == PresenceStatus::Off {
                player.discord = PresenceStatus::Unavailable;
            }
            self.submitted = Some(desired);
        }
        if let Some(worker) = worker {
            for (revision, status) in worker.status.try_iter() {
                if revision == self.revision {
                    player.discord = status;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, thread};

    #[test]
    fn exports_written_before_the_connection_log_are_read_after_it() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("mock-uxplay");
        let connection = directory.path().join("connect");
        let script = format!(
            "#!/bin/sh\n\
             while [ \"$1\" != -md ]; do shift; done\n\
             printf 'Title: Early Song\\nArtist: Test Artist\\n\\000' > \"$2\"\n\
             printf 'register_dnssd: advertised AirPlay service\\n'\n\
             while [ ! -e '{}' ]; do sleep 0.02; done\n\
             printf 'Open connections: 1\\n'\n\
             exec sleep 10\n",
            connection.display()
        );
        fs::write(&binary, script).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let mut receiver = Receiver::spawn(&binary, "Test").unwrap();
        let dacp = dacp::Worker::spawn().unwrap();
        let mut session = Session::default();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !session.player.ready && Instant::now() < deadline {
            session
                .poll_receiver(&mut receiver, &dacp, Instant::now())
                .unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        assert!(session.player.ready);
        assert!(session.player.track.is_none());
        fs::write(connection, b"").unwrap();
        while !session.player.connected && Instant::now() < deadline {
            session
                .poll_receiver(&mut receiver, &dacp, Instant::now())
                .unwrap();
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            session
                .player
                .track
                .as_ref()
                .map(|track| track.title.as_str()),
            Some("Early Song")
        );
    }
}
