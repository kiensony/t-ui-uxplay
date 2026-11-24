mod app;
mod dacp;
mod discord;
mod lyrics;
mod receiver;
mod settings;
mod spectrum;
mod state;
mod terminal;
mod ui;

#[cfg(test)]
mod acceptance;

use anyhow::{Context, Result, bail};
use app::App;
use clap::Parser;
use crossterm::event::{self, Event, MouseButton, MouseEventKind};
use receiver::{LogEvent, Receiver};
use settings::{Overrides, Store};
#[cfg(unix)]
use signal_hook::{
    consts::signal::{SIGHUP, SIGINT, SIGTERM},
    flag,
};
use state::{Player, PresenceStatus};
#[cfg(unix)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use terminal::PlayerTerminal;
use ui::Action;

#[derive(Parser)]
#[command(version, about)]
struct Options {
    /// Name shown in the sender's AirPlay audio picker
    #[arg(long, value_parser = receiver_name)]
    name: Option<String>,

    /// UxPlay executable or path. Defaults to the bundled receiver, then PATH.
    #[arg(long)]
    uxplay: Option<PathBuf>,

    /// Application ID from the Discord Developer Portal
    #[arg(long, value_parser = application_id)]
    discord_app_id: Option<String>,

    /// Keep track details local and skip LRCLIB lookups
    #[arg(long)]
    no_online_lyrics: bool,
}

fn receiver_name(value: &str) -> Result<String, &'static str> {
    if value.chars().any(char::is_control) {
        return Err("Speaker names cannot contain control characters");
    }
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') {
        return Err("Enter a speaker name without a leading hyphen");
    }
    if value.len() > 50 {
        return Err("Speaker names can use up to 50 UTF-8 bytes");
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
    #[cfg(unix)]
    let stop = {
        let stop = Arc::new(AtomicBool::new(false));
        for signal in [SIGINT, SIGTERM, SIGHUP] {
            flag::register(signal, stop.clone())?;
        }
        stop
    };
    let mut terminal = PlayerTerminal::open()?;
    let settings = Store::load(Overrides {
        name: options.name,
        no_online_lyrics: options.no_online_lyrics,
    });
    let mut app = App::new(settings, options.uxplay, Instant::now())?;
    let discord = options
        .discord_app_id
        .map(discord::Worker::spawn)
        .transpose()?;

    let mut sharing = Sharing::default();
    loop {
        #[cfg(unix)]
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let now = Instant::now();
        app.poll(now);
        sharing.sync(&mut app.session.player, discord.as_ref(), now);
        let screen = terminal.draw(
            &app.session.player,
            app.settings.name(),
            now,
            app.dialog.as_ref(),
        )?;
        let frame_ms = if app.session.player.visualizer.is_some() {
            42
        } else {
            100
        };
        if !event::poll(Duration::from_millis(frame_ms))? {
            continue;
        }
        let action = match event::read()? {
            Event::Key(key) => screen.key(key),
            Event::Mouse(mouse) if mouse.kind == MouseEventKind::Down(MouseButton::Left) => {
                screen.click(mouse.column, mouse.row)
            }
            Event::Resize(..) => {
                terminal.resized()?;
                None
            }
            Event::Paste(text) => {
                if let Some(dialog) = &mut app.dialog {
                    dialog.insert_name(&text);
                }
                None
            }
            _ => None,
        };
        match action {
            Some(Action::Quit) => break,
            Some(Action::Transport(command)) => app.transport(command),
            Some(Action::OpenSettings) => app.open_settings(),
            Some(Action::Settings(input)) => app.settings_input(input, Instant::now()),
            Some(Action::Discord) => {
                sharing.enabled = !sharing.enabled;
                sharing.sync(&mut app.session.player, discord.as_ref(), Instant::now());
            }
            Some(Action::CycleVisualizer) => app.session.player.cycle_visualizer(),
            Some(Action::Scroll(delta)) => {
                let offset = screen
                    .scroll_top
                    .saturating_add_signed(isize::from(delta))
                    .min(screen.scroll_max);
                if screen.scroll_max > 0 {
                    app.session.player.scroll_lyrics(offset, now);
                }
            }
            Some(Action::AdjustLyricSync(delta_ms)) => {
                app.session.player.adjust_lyric_sync(delta_ms)
            }
            None => {}
        }
    }
    Ok(())
}

#[derive(Default)]
struct Session {
    player: Player,
    sender_connected: bool,
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
    ) -> Result<Option<Instant>> {
        let exit = receiver
            .exit_status()
            .context("Could not check UxPlay status")?;
        let mut progress = None;
        let mut advertised = None;
        for log in receiver.logs.try_iter() {
            match log.event {
                LogEvent::Ready => {
                    self.player.ready = true;
                    advertised.get_or_insert(log.received_at);
                }
                LogEvent::Connected => {
                    self.sender_connected = true;
                    self.player.connected = true;
                }
                LogEvent::StreamStarted => {
                    self.sender_connected = true;
                    self.player.stream_started(now);
                    receiver.spectrum.clear();
                    progress = None;
                }
                LogEvent::StreamStopped | LogEvent::Disconnected => {
                    if log.event == LogEvent::Disconnected {
                        self.sender_connected = false;
                    }
                    self.player.disconnect(now);
                    receiver.spectrum.clear();
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
        Ok(advertised)
    }

    fn reset_receiver(&mut self, dacp: &dacp::Worker, now: Instant) {
        self.player.disconnect(now);
        self.player.ready = false;
        self.player.receiver_unavailable = false;
        self.player.reconnect = false;
        self.player.spectrum = None;
        self.sender_connected = false;
        self.diagnostic = None;
        self.set_credentials(dacp, None);
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

#[cfg(all(test, unix))]
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
             printf 'register_dnssd: advertised AirPlay service with \"Features\" code = 0x527FFEE6\\n'\n\
             while [ ! -e '{}' ]; do sleep 0.02; done\n\
             printf 'Open connections: 1\\n'\n\
             exec sleep 10\n",
            connection.display()
        );
        fs::write(&binary, script).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let mut receiver = Receiver::start(std::process::Command::new(&binary), "Test").unwrap();
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
