use crate::{
    Session, dacp, lyrics,
    receiver::Receiver,
    settings::{Dialog, Input, Intent, Mode, Settings, Store},
};
use anyhow::Result;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

pub struct App {
    pub session: Session,
    pub settings: Store,
    pub dialog: Option<Dialog>,
    receiver: Option<Receiver>,
    executable: Option<PathBuf>,
    startup: Option<Startup>,
    receiver_error: Option<String>,
    dacp: dacp::Worker,
    lyrics: Option<lyrics::Worker>,
}

struct Startup {
    goal: Goal,
    deadline: Instant,
}

enum Goal {
    Save(Settings),
    Restore(Option<String>),
}

impl App {
    pub fn new(settings: Store, executable: Option<PathBuf>, now: Instant) -> Result<Self> {
        let mut session = Session::default();
        session.player.visualizer = settings.saved.view.visualizer();
        session.player.set_online_lyrics(settings.online_lyrics());
        let lyrics = settings
            .online_lyrics()
            .then(lyrics::Worker::spawn)
            .transpose()?;
        let mut app = Self {
            session,
            settings,
            dialog: None,
            receiver: None,
            executable,
            startup: None,
            receiver_error: None,
            dacp: dacp::Worker::spawn()?,
            lyrics,
        };
        app.start(app.settings.name().to_owned(), None, now);
        Ok(app)
    }

    pub fn poll(&mut self, now: Instant) {
        self.poll_receiver(now);
        let player = &mut self.session.player;
        player.spectrum = self
            .receiver
            .as_ref()
            .filter(|_| player.connected && player.visualizer.is_some())
            .and_then(|receiver| receiver.spectrum.latest(now));
        if let Some(worker) = &self.lyrics {
            if let Some(request) = player.lyrics_request(now) {
                worker.request(request);
            }
            for (request, result) in worker.results.try_iter() {
                player.set_lyrics(&request, result);
            }
        }
    }

    pub fn transport(&self, command: dacp::Command) {
        if self.dialog.is_none() && self.session.player.transport {
            self.dacp.control(self.session.transport_revision, command);
        }
    }

    pub fn open_settings(&mut self) {
        let mut dialog = Dialog::new(&self.settings);
        if let Some(error) = &self.receiver_error {
            dialog.error = Some(match dialog.error.take() {
                Some(settings_error) => format!("{error}\n{settings_error}"),
                None => error.clone(),
            });
        }
        if matches!(
            self.startup.as_ref().map(|start| &start.goal),
            Some(Goal::Restore(_))
        ) {
            dialog.mode = Mode::Restoring;
        }
        self.dialog = Some(dialog);
    }

    pub fn settings_input(&mut self, input: Input, now: Instant) {
        let Some(intent) = self.dialog.as_mut().and_then(|dialog| dialog.edit(input)) else {
            return;
        };
        if intent == Intent::Dismiss {
            self.dialog = None;
            if matches!(
                self.startup.as_ref().map(|start| &start.goal),
                Some(Goal::Save(_))
            ) {
                self.restore(None, now);
            }
            return;
        }
        let Some(dialog) = &mut self.dialog else {
            return;
        };
        let draft = match dialog.draft.clone().validated() {
            Ok(draft) => draft,
            Err(error) => {
                dialog.error = Some(error.to_string());
                return;
            }
        };

        // Process connections reported while the terminal was waiting for input.
        self.poll_receiver(now);
        let name = self.settings.name_for(&draft);
        if name != self.settings.name() || self.receiver.is_none() {
            if self.session.sender_connected && intent == Intent::Save {
                if let Some(dialog) = &mut self.dialog {
                    dialog.mode = Mode::Confirm { rename: false };
                }
            } else {
                let name = name.to_owned();
                self.start(name, Some(Goal::Save(draft)), now);
            }
        } else if let Err(error) = self.commit(draft) {
            self.edit_error(format!("{error:#}"));
        }
    }

    fn start(&mut self, name: String, goal: Option<Goal>, now: Instant) {
        self.reset_receiver(now);
        if let Some(dialog) = &mut self.dialog {
            match &goal {
                None => {}
                Some(Goal::Save(_)) => dialog.mode = Mode::Starting,
                Some(Goal::Restore(_)) => dialog.mode = Mode::Restoring,
            }
        }
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        match Receiver::spawn(self.executable.as_deref(), &name) {
            Ok(receiver) => {
                self.receiver = Some(receiver);
                self.startup = goal.map(|goal| Startup { goal, deadline });
            }
            Err(error) => self.failed(goal, format!("{error:#}"), now),
        }
    }

    fn reset_receiver(&mut self, now: Instant) {
        self.startup = None;
        self.receiver_error = None;
        self.session.reset_receiver(&self.dacp, now);
        if let Some(lyrics) = &self.lyrics {
            lyrics.cancel();
        }
        self.receiver = None;
    }

    fn poll_receiver(&mut self, now: Instant) {
        let Some(receiver) = &mut self.receiver else {
            return;
        };
        let advertised = match self.session.poll_receiver(receiver, &self.dacp, now) {
            Ok(advertised) => advertised,
            Err(error) => {
                let goal = self.startup.take().map(|pending| pending.goal);
                self.failed(goal, format!("{error:#}"), now);
                return;
            }
        };
        if self.session.player.connected {
            self.session.player.reconnect = false;
        }
        let Some(pending) = &self.startup else { return };
        let ready = advertised.is_some_and(|at| at <= pending.deadline);
        if !ready && now < pending.deadline && advertised.is_none() {
            return;
        }
        let goal = self.startup.take().unwrap().goal;
        if ready {
            self.started(goal, now);
        } else {
            let mut error = "UxPlay did not advertise AirPlay within 10 seconds".to_owned();
            if let Some(detail) = &self.session.diagnostic {
                error.push_str(&format!(". {detail}"));
            }
            self.failed(Some(goal), error, now);
        }
    }

    fn started(&mut self, goal: Goal, now: Instant) {
        match goal {
            Goal::Save(draft) => match self.commit(draft) {
                Ok(()) => self.session.player.reconnect = !self.session.player.connected,
                Err(error) => self.restore(Some(format!("{error:#}")), now),
            },
            Goal::Restore(error) => {
                self.session.player.reconnect = !self.session.player.connected;
                if let Some(error) = error {
                    self.edit_error(format!("{error}\nPrevious receiver restored."));
                } else if let Some(dialog) = &mut self.dialog {
                    dialog.mode = Mode::Edit;
                }
            }
        }
    }

    fn failed(&mut self, goal: Option<Goal>, error: String, now: Instant) {
        let error = match goal {
            Some(Goal::Save(_)) => {
                self.restore(Some(error), now);
                return;
            }
            Some(Goal::Restore(Some(save_error))) => {
                format!("{save_error}\nCould not restore the previous receiver. {error}")
            }
            _ => error,
        };
        self.reset_receiver(now);
        self.session.player.receiver_unavailable = true;
        self.receiver_error = Some(error.clone());
        self.edit_error(error);
    }

    fn restore(&mut self, error: Option<String>, now: Instant) {
        if let Some(dialog) = &mut self.dialog {
            dialog.error.clone_from(&error);
        }
        self.start(
            self.settings.name().to_owned(),
            Some(Goal::Restore(error)),
            now,
        );
    }

    fn edit_error(&mut self, error: String) {
        if let Some(dialog) = &mut self.dialog {
            dialog.mode = Mode::Edit;
            dialog.error = Some(error);
        }
    }

    fn commit(&mut self, draft: Settings) -> Result<()> {
        let previous_view = self.settings.saved.view;
        let worker = (self.settings.online_lyrics_for(&draft) && self.lyrics.is_none())
            .then(lyrics::Worker::spawn)
            .transpose()?;
        self.settings.save(draft)?;
        if let Some(worker) = worker {
            self.lyrics = Some(worker);
        }
        if !self.settings.online_lyrics()
            && let Some(lyrics) = &self.lyrics
        {
            lyrics.cancel();
        }
        self.session
            .player
            .set_online_lyrics(self.settings.online_lyrics());
        if previous_view != self.settings.saved.view {
            self.session.player.visualizer = self.settings.saved.view.visualizer();
        }
        self.dialog = None;
        Ok(())
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{
        settings::{self, Overrides},
        state::Track,
    };
    use std::{fs, os::unix::fs::PermissionsExt, thread};

    struct Fixture {
        directory: tempfile::TempDir,
        path: PathBuf,
        app: App,
    }

    impl Fixture {
        fn new() -> Self {
            Self::with_sender(true, false)
        }

        fn with_sender(connected: bool, stream_stopped: bool) -> Self {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("settings.json");
            let binary = directory.path().join("mock-uxplay");
            let body = r#"#!/bin/sh
cd "$(dirname "$0")"
while [ "$1" != -n ]; do shift; done
case "$2" in
    "音楽 Lounge")
        printf 'Initialized server socket(s)\n'
        while [ ! -f advertise ]; do sleep 0.01; done
        ;;
    Fail) printf 'dnssd_register_airplay failed\n'; exit 1 ;;
    Timeout) exec sleep 60 ;;
    Old) if [ -f old-fails ]; then exit 1; fi ;;
esac
printf 'register_dnssd: advertised AirPlay service with "Features" code = 0x527FFEE6\n'
if [ "$2" = Old ] && [ -f connected ]; then
    printf 'Open connections: 1\n'
    if [ -f stream-stopped ]; then printf 'TEARDOWN request,  96=1, 110=0\n'; fi
fi
exec sleep 60
"#;
            fs::write(&binary, body).unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
            let mut store = Store::open(Ok(path.clone()), Overrides::default());
            store
                .save(Settings {
                    name: "Old".into(),
                    online_lyrics: false,
                    ..Settings::default()
                })
                .unwrap();
            if connected {
                fs::write(directory.path().join("connected"), "").unwrap();
            }
            if stream_stopped {
                fs::write(directory.path().join("stream-stopped"), "").unwrap();
            }
            let app = App::new(store, Some(binary), Instant::now()).unwrap();
            let mut fixture = Self {
                directory,
                path,
                app,
            };
            fixture.poll_until(|app| {
                app.session.player.ready
                    && app.session.player.connected == (connected && !stream_stopped)
            });
            fixture
        }

        fn poll_until(&mut self, done: impl Fn(&App) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !done(&self.app) {
                assert!(
                    Instant::now() < deadline,
                    "receiver did not reach the expected state"
                );
                self.app.poll(Instant::now());
                thread::sleep(Duration::from_millis(5));
            }
        }

        fn input(&mut self, input: Input) {
            self.app.settings_input(input, Instant::now());
        }

        fn draft_name(&mut self, name: &str) {
            self.app.open_settings();
            self.app.dialog.as_mut().unwrap().draft.name = name.into();
            self.input(Input::Activate(settings::Field::Save));
        }

        fn saved_name(&self) -> String {
            Store::open(Ok(self.path.clone()), Overrides::default())
                .name()
                .to_owned()
        }
    }

    #[test]
    fn a_sender_with_a_stopped_stream_still_requires_confirmation() {
        let mut fixture = Fixture::with_sender(true, true);
        fixture.draft_name("音楽 Lounge");
        assert!(matches!(
            fixture.app.dialog.as_ref().unwrap().mode,
            Mode::Confirm { rename: false }
        ));
        assert_eq!(fixture.saved_name(), "Old");
    }

    #[test]
    fn an_idle_rename_starts_without_confirmation() {
        let mut fixture = Fixture::with_sender(false, false);
        fs::write(fixture.directory.path().join("advertise"), "").unwrap();
        fixture.draft_name("音楽 Lounge");
        assert!(fixture.app.dialog.as_ref().unwrap().mode == Mode::Starting);
        fixture.poll_until(|app| app.dialog.is_none());
        assert_eq!(fixture.saved_name(), "音楽 Lounge");
    }

    #[test]
    fn connected_rename_requires_confirmation_and_waits_for_advertisement() {
        let mut fixture = Fixture::new();
        let now = Instant::now();
        fixture.app.session.player.set_track(
            Some(Track {
                title: "Old song".into(),
                ..Track::default()
            }),
            now,
        );
        fixture.app.session.player.artwork = Some(image::DynamicImage::new_rgb8(1, 1));
        fixture.app.session.player.transport = true;
        fixture.draft_name("音楽 Lounge");
        assert!(fixture.app.session.player.connected);
        assert_eq!(fixture.saved_name(), "Old");
        fixture.input(Input::Escape);
        assert!(fixture.app.dialog.is_none());
        assert_eq!(fixture.saved_name(), "Old");
        assert!(fixture.app.session.player.track.is_some());

        fixture.draft_name("音楽 Lounge");
        fixture.input(Input::Confirm(true));
        let player = &fixture.app.session.player;
        assert!(!player.connected && !player.transport);
        assert!(player.track.is_none() && player.artwork.is_none());
        assert_eq!(fixture.saved_name(), "Old");
        fs::write(fixture.directory.path().join("advertise"), "").unwrap();
        fixture.poll_until(|app| app.dialog.is_none());
        assert_eq!(fixture.app.settings.name(), "音楽 Lounge");
        assert_eq!(fixture.saved_name(), "音楽 Lounge");
        assert!(fixture.app.session.player.reconnect);
    }

    #[test]
    fn failed_start_and_timeout_restore_the_previous_receiver_with_the_draft_open() {
        for name in ["Fail", "Timeout"] {
            let mut fixture = Fixture::new();
            fixture.draft_name(name);
            fixture.input(Input::Confirm(true));
            if name == "Timeout" {
                fixture
                    .app
                    .poll(fixture.app.startup.as_ref().unwrap().deadline);
            }
            fixture.poll_until(|app| app.session.player.ready && app.startup.is_none());
            assert_eq!(fixture.saved_name(), "Old");
            let dialog = fixture.app.dialog.as_ref().unwrap();
            assert_eq!(dialog.draft.name, name);
            let error = dialog.error.as_deref().unwrap();
            assert!(
                error.contains(if name == "Timeout" {
                    "10 seconds"
                } else {
                    "dnssd_register_airplay failed"
                }),
                "{error}"
            );
        }
    }

    #[test]
    fn write_failure_rolls_back_and_an_unavailable_receiver_can_be_retried() {
        let mut fixture = Fixture::new();
        fs::write(fixture.directory.path().join("old-fails"), "").unwrap();
        fs::remove_file(&fixture.path).unwrap();
        fs::create_dir(&fixture.path).unwrap();
        fs::write(fixture.directory.path().join("advertise"), "").unwrap();
        fixture.draft_name("音楽 Lounge");
        fixture.input(Input::Confirm(true));
        fixture.poll_until(|app| app.session.player.receiver_unavailable);
        assert_eq!(fixture.app.settings.name(), "Old");
        assert!(
            fixture
                .app
                .dialog
                .as_ref()
                .unwrap()
                .error
                .as_deref()
                .unwrap()
                .contains("Could not save settings")
        );
        fs::remove_dir(&fixture.path).unwrap();
        fixture.input(Input::Activate(settings::Field::Save));
        fixture.poll_until(|app| app.dialog.is_none());
        assert_eq!(fixture.saved_name(), "音楽 Lounge");
        assert!(fixture.app.session.player.ready);
    }

    #[test]
    fn non_name_saves_preserve_the_active_receiver() {
        let mut fixture = Fixture::new();
        fixture.app.session.player.set_track(
            Some(Track {
                title: "Live".into(),
                ..Track::default()
            }),
            Instant::now(),
        );
        fixture.app.open_settings();
        fixture.input(Input::Activate(settings::Field::View));
        fixture.input(Input::Activate(settings::Field::Lyrics));
        fixture.input(Input::Activate(settings::Field::DiscordAppId));
        fixture.app.dialog.as_mut().unwrap().insert_text("12345");
        fixture.input(Input::Activate(settings::Field::Save));
        assert!(fixture.app.dialog.is_none());
        assert!(fixture.app.session.player.connected);
        assert_eq!(
            fixture.app.session.player.track.as_ref().unwrap().title,
            "Live"
        );
        assert_eq!(
            fixture.app.session.player.visualizer,
            Some(crate::state::Visualizer::Ribbons)
        );
        assert!(!fixture.app.session.player.lyrics_disabled);
        fixture.app.session.player.cycle_visualizer();
        let reloaded = Store::open(Ok(fixture.path.clone()), Overrides::default());
        assert_eq!(reloaded.saved.view, settings::View::Ribbons);
        assert_eq!(reloaded.discord_app_id(), Some("12345"));
    }
}
