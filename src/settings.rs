use crate::{application_id, receiver_name, state::Visualizer};
use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::{
    env,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    #[default]
    Cover,
    Ribbons,
    Spectrum,
}

impl View {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cover => "Cover",
            Self::Ribbons => "Ribbons",
            Self::Spectrum => "Spectrum",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Cover => Self::Ribbons,
            Self::Ribbons => Self::Spectrum,
            Self::Spectrum => Self::Cover,
        }
    }

    pub fn visualizer(self) -> Option<Visualizer> {
        match self {
            Self::Cover => None,
            Self::Ribbons => Some(Visualizer::Ribbons),
            Self::Spectrum => Some(Visualizer::Spectrum),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Palette {
    #[default]
    Default,
    FemboyTransgirl,
}

impl Palette {
    pub fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::FemboyTransgirl => "Femboy / Transgirl",
        }
    }

    fn next(self) -> Self {
        match self {
            Self::Default => Self::FemboyTransgirl,
            Self::FemboyTransgirl => Self::Default,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Settings {
    pub name: String,
    pub online_lyrics: bool,
    pub view: View,
    #[serde(default)]
    pub palette: Palette,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discord_app_id: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            name: "t-ui-uxplay".into(),
            online_lyrics: true,
            view: View::Cover,
            palette: Palette::default(),
            discord_app_id: None,
        }
    }
}

impl Settings {
    pub fn validated(mut self) -> Result<Self> {
        self.name = receiver_name(&self.name).map_err(anyhow::Error::msg)?;
        self.discord_app_id = self
            .discord_app_id
            .as_deref()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(application_id)
            .transpose()
            .map_err(anyhow::Error::msg)?;
        Ok(self)
    }
}

#[derive(Clone, Default)]
pub struct Overrides {
    pub name: Option<String>,
    pub no_online_lyrics: bool,
    pub discord_app_id: Option<String>,
}

impl Overrides {
    fn name<'a>(&'a self, settings: &'a Settings) -> &'a str {
        self.name.as_deref().unwrap_or(&settings.name)
    }

    fn online_lyrics(&self, settings: &Settings) -> bool {
        !self.no_online_lyrics && settings.online_lyrics
    }

    fn discord_app_id<'a>(&'a self, settings: &'a Settings) -> Option<&'a str> {
        self.discord_app_id
            .as_deref()
            .or(settings.discord_app_id.as_deref())
    }
}

pub struct Store {
    path: Result<PathBuf, String>,
    pub saved: Settings,
    pub error: Option<String>,
    overrides: Overrides,
}

impl Store {
    pub fn load(overrides: Overrides) -> Self {
        let path = config_home()
            .map(|dir| dir.join("t-ui-uxplay").join("settings.json"))
            .ok_or_else(|| "Could not find the user config directory".to_owned());
        Self::open(path, overrides)
    }

    pub fn open(path: Result<PathBuf, String>, overrides: Overrides) -> Self {
        let loaded = match &path {
            Ok(path) => read(path).map_err(|error| format!("Could not load settings: {error:#}")),
            Err(error) => Err(error.clone()),
        };
        let (saved, error) = match loaded {
            Ok(saved) => (saved, None),
            Err(error) => (
                Settings {
                    online_lyrics: false,
                    ..Settings::default()
                },
                Some(error),
            ),
        };
        Self {
            path,
            saved,
            error,
            overrides,
        }
    }

    pub fn name(&self) -> &str {
        self.name_for(&self.saved)
    }

    pub fn name_for<'a>(&'a self, settings: &'a Settings) -> &'a str {
        self.overrides.name(settings)
    }

    pub fn online_lyrics(&self) -> bool {
        self.online_lyrics_for(&self.saved)
    }

    pub fn online_lyrics_for(&self, settings: &Settings) -> bool {
        self.overrides.online_lyrics(settings)
    }

    pub fn discord_app_id(&self) -> Option<&str> {
        self.overrides.discord_app_id(&self.saved)
    }

    pub fn save(&mut self, draft: Settings) -> Result<()> {
        let draft = draft.validated()?;
        let path = self
            .path
            .as_ref()
            .map_err(|error| anyhow::Error::msg(error.clone()))?;
        write(path, &draft).context("Could not save settings")?;
        self.saved = draft;
        self.error = None;
        Ok(())
    }
}

fn config_home() -> Option<PathBuf> {
    if cfg!(windows) {
        absolute_env("APPDATA").or_else(|| {
            absolute_env("USERPROFILE").map(|home| home.join("AppData").join("Roaming"))
        })
    } else if cfg!(target_os = "macos") {
        absolute_env("HOME").map(|home| home.join("Library").join("Application Support"))
    } else {
        absolute_env("XDG_CONFIG_HOME")
            .or_else(|| absolute_env("HOME").map(|home| home.join(".config")))
    }
}

fn absolute_env(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
}

fn read(path: &Path) -> Result<Settings> {
    match File::open(path) {
        Ok(file) => serde_json::from_reader::<_, Settings>(io::BufReader::new(file))?.validated(),
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                && matches!(fs::symlink_metadata(path), Err(metadata) if metadata.kind() == io::ErrorKind::NotFound) =>
        {
            Ok(Settings::default())
        }
        Err(error) => Err(error.into()),
    }
}

fn write(path: &Path, settings: &Settings) -> Result<()> {
    let directory = path
        .parent()
        .context("Settings path has no parent directory")?;
    fs::create_dir_all(directory)?;
    let mut temp = NamedTempFile::new_in(directory)?;
    serde_json::to_writer_pretty(&mut temp, settings)?;
    temp.write_all(b"\n")?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Lyrics,
    View,
    Palette,
    DiscordAppId,
    Save,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Edit,
    Confirm { rename: bool },
    Starting,
    Restoring,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Tab(bool),
    Enter,
    Escape,
    Character(char),
    Backspace,
    Activate(Field),
    Confirm(bool),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Intent {
    Save,
    Rename,
    Dismiss,
}

pub struct Dialog {
    pub draft: Settings,
    pub focus: Field,
    pub mode: Mode,
    pub error: Option<String>,
    pub text_selected: bool,
    overrides: Overrides,
}

impl Dialog {
    pub fn new(store: &Store) -> Self {
        let mut dialog = Self {
            draft: store.saved.clone(),
            focus: Field::Name,
            mode: Mode::Edit,
            error: store.error.clone(),
            text_selected: true,
            overrides: store.overrides.clone(),
        };
        let focus = dialog.fields().next().unwrap_or(Field::Save);
        dialog.focus = focus;
        dialog
    }

    pub fn name(&self) -> &str {
        self.overrides.name(&self.draft)
    }

    pub fn online_lyrics(&self) -> bool {
        self.overrides.online_lyrics(&self.draft)
    }

    pub fn discord_app_id(&self) -> Option<&str> {
        self.overrides.discord_app_id(&self.draft)
    }

    pub fn locked(&self, field: Field) -> bool {
        match field {
            Field::Name => self.overrides.name.is_some(),
            Field::Lyrics => self.overrides.no_online_lyrics,
            Field::DiscordAppId => self.overrides.discord_app_id.is_some(),
            Field::View | Field::Palette | Field::Save => false,
        }
    }

    fn fields(&self) -> impl Iterator<Item = Field> + '_ {
        [
            Field::Name,
            Field::Lyrics,
            Field::View,
            Field::Palette,
            Field::DiscordAppId,
            Field::Save,
        ]
        .into_iter()
        .filter(|&field| !self.locked(field))
    }

    pub fn key(key: KeyEvent) -> Option<Input> {
        let alt_graph = KeyModifiers::CONTROL | KeyModifiers::ALT;
        match (key.code, key.modifiers) {
            (KeyCode::BackTab, KeyModifiers::NONE | KeyModifiers::SHIFT)
            | (KeyCode::Tab, KeyModifiers::SHIFT) => Some(Input::Tab(true)),
            (KeyCode::Tab, KeyModifiers::NONE) => Some(Input::Tab(false)),
            (KeyCode::Enter, KeyModifiers::NONE) => Some(Input::Enter),
            (KeyCode::Esc, KeyModifiers::NONE) => Some(Input::Escape),
            (KeyCode::Backspace, KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                Some(Input::Backspace)
            }
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                Some(Input::Character(c))
            }
            (KeyCode::Char(c), modifiers) if modifiers - KeyModifiers::SHIFT == alt_graph => {
                Some(Input::Character(c))
            }
            _ => None,
        }
    }

    pub fn edit(&mut self, input: Input) -> Option<Intent> {
        match self.mode {
            _ if input == Input::Escape => Some(Intent::Dismiss),
            Mode::Starting | Mode::Restoring => None,
            Mode::Confirm { rename } => {
                let rename = match input {
                    Input::Enter => rename,
                    Input::Confirm(choice) => choice,
                    Input::Tab(_) => {
                        self.mode = Mode::Confirm { rename: !rename };
                        return None;
                    }
                    _ => return None,
                };
                if rename {
                    Some(Intent::Rename)
                } else {
                    self.mode = Mode::Edit;
                    None
                }
            }
            Mode::Edit => {
                match input {
                    Input::Tab(back) => {
                        let fields: Vec<_> = self.fields().collect();
                        let index = fields
                            .iter()
                            .position(|&field| field == self.focus)
                            .unwrap_or(0);
                        let offset = if back { fields.len() - 1 } else { 1 };
                        self.focus = fields[(index + offset) % fields.len()];
                        self.text_selected = true;
                    }
                    Input::Enter => return self.activate(self.focus),
                    Input::Activate(field) => return self.activate(field),
                    Input::Character(c) if !c.is_control() => self.insert_text(&c.to_string()),
                    Input::Backspace => {
                        let selected = self.text_selected;
                        if let Some(value) = self.text_draft() {
                            if selected {
                                value.clear();
                            } else if let Some((index, _)) =
                                value.grapheme_indices(true).next_back()
                            {
                                value.truncate(index);
                            }
                            self.text_selected = false;
                        }
                    }
                    _ => {}
                }
                None
            }
        }
    }

    pub fn insert_text(&mut self, text: &str) {
        if self.mode != Mode::Edit {
            return;
        }
        let field = self.focus;
        let selected = self.text_selected;
        let Some(value) = self.text_draft() else {
            return;
        };
        if text.chars().any(char::is_control) {
            self.error = Some(if field == Field::Name {
                "Speaker names cannot contain control characters".into()
            } else {
                "Use the numeric Discord application ID".into()
            });
        } else {
            if selected {
                value.clear();
            }
            value.push_str(text);
            self.text_selected = false;
        }
    }

    fn text_draft(&mut self) -> Option<&mut String> {
        if self.locked(self.focus) {
            return None;
        }
        match self.focus {
            Field::Name => Some(&mut self.draft.name),
            Field::DiscordAppId => Some(self.draft.discord_app_id.get_or_insert_default()),
            _ => None,
        }
    }

    fn activate(&mut self, field: Field) -> Option<Intent> {
        if self.locked(field) {
            return None;
        }
        self.focus = field;
        match field {
            Field::Name | Field::DiscordAppId => self.text_selected = true,
            Field::Lyrics => self.draft.online_lyrics = !self.draft.online_lyrics,
            Field::View => self.draft.view = self.draft.view.next(),
            Field::Palette => self.draft.palette = self.draft.palette.next(),
            Field::Save => return Some(Intent::Save),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_defaults_for_older_settings_and_round_trips_through_the_dialog() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(
            &path,
            r#"{"name":"Home","online_lyrics":false,"view":"ribbons"}"#,
        )
        .unwrap();
        let mut store = Store::open(Ok(path.clone()), Overrides::default());
        for (initial, next) in [
            (Palette::Default, Palette::FemboyTransgirl),
            (Palette::FemboyTransgirl, Palette::Default),
        ] {
            assert!(store.error.is_none());
            assert_eq!(store.saved.palette, initial);
            let mut dialog = Dialog::new(&store);
            for _ in 0..3 {
                dialog.edit(Input::Tab(false));
            }
            dialog.edit(Input::Enter);
            assert_eq!(dialog.draft.palette, next);
            assert_eq!(store.saved.palette, initial);
            store.save(dialog.draft).unwrap();
            store = Store::open(Ok(path.clone()), Overrides::default());
            assert_eq!(store.saved.palette, next);
            assert_eq!(store.name(), "Home");
            assert_eq!(store.saved.view, View::Ribbons);
            assert!(!store.online_lyrics());
        }
    }

    #[test]
    fn discord_application_id_can_be_edited_saved_and_cleared() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(
            &path,
            r#"{"name":"Home","online_lyrics":true,"view":"cover"}"#,
        )
        .unwrap();
        let mut store = Store::open(Ok(path.clone()), Overrides::default());
        assert!(store.error.is_none());
        assert_eq!(store.discord_app_id(), None);

        let mut dialog = Dialog::new(&store);
        dialog.edit(Input::Activate(Field::DiscordAppId));
        dialog.insert_text(" 12345 ");
        store.save(dialog.draft).unwrap();
        let mut store = Store::open(Ok(path.clone()), Overrides::default());
        assert_eq!(store.discord_app_id(), Some("12345"));

        let mut dialog = Dialog::new(&store);
        dialog.edit(Input::Activate(Field::DiscordAppId));
        dialog.edit(Input::Character('x'));
        assert!(store.save(dialog.draft).is_err());
        assert_eq!(store.discord_app_id(), Some("12345"));

        let mut dialog = Dialog::new(&store);
        dialog.edit(Input::Activate(Field::DiscordAppId));
        dialog.edit(Input::Backspace);
        store.save(dialog.draft).unwrap();
        let store = Store::open(Ok(path), Overrides::default());
        assert_eq!(store.discord_app_id(), None);
    }

    #[test]
    fn settings_survive_reload_and_launch_overrides_do_not_replace_saved_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("t-ui-uxplay/settings.json");
        let mut store = Store::open(Ok(path.clone()), Overrides::default());
        assert!(store.online_lyrics());
        assert_eq!(store.saved.view, View::Cover);
        store
            .save(Settings {
                name: "  ห้อง 音楽  ".into(),
                online_lyrics: true,
                view: View::Ribbons,
                palette: Palette::default(),
                discord_app_id: Some("12345".into()),
            })
            .unwrap();
        let mut store = Store::open(
            Ok(path.clone()),
            Overrides {
                name: Some("CLI Speaker".into()),
                no_online_lyrics: true,
                discord_app_id: Some("67890".into()),
            },
        );
        assert_eq!(store.name(), "CLI Speaker");
        assert!(!store.online_lyrics());
        let mut dialog = Dialog::new(&store);
        assert_eq!(dialog.focus, Field::View);
        assert_eq!(dialog.name(), "CLI Speaker");
        assert!(!dialog.online_lyrics());
        assert!(dialog.locked(Field::DiscordAppId));
        assert_eq!(dialog.discord_app_id(), Some("67890"));
        dialog.edit(Input::Activate(Field::Lyrics));
        dialog.edit(Input::Activate(Field::DiscordAppId));
        dialog.insert_text("11111");
        dialog.edit(Input::Activate(Field::View));
        store.save(dialog.draft).unwrap();

        let store = Store::open(Ok(path), Overrides::default());
        assert_eq!(store.name(), "ห้อง 音楽");
        assert!(store.online_lyrics());
        assert_eq!(store.saved.view, View::Spectrum);
        assert_eq!(store.discord_app_id(), Some("12345"));
    }

    #[test]
    fn malformed_or_unreadable_settings_require_a_valid_save_to_recover() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        std::fs::create_dir(&path).unwrap();
        let mut store = Store::open(Ok(path.clone()), Overrides::default());
        assert!(store.error.is_some());
        assert!(!store.online_lyrics());
        let original = store.saved.clone();
        assert!(store.save(Settings::default()).is_err());
        assert_eq!(store.saved, original);

        std::fs::remove_dir(&path).unwrap();
        for contents in [
            "{",
            r#"{"name":"-Bad","online_lyrics":true,"view":"cover"}"#,
        ] {
            std::fs::write(&path, contents).unwrap();
            let store = Store::open(Ok(path.clone()), Overrides::default());
            assert!(store.error.is_some());
            assert!(!store.online_lyrics());
        }
        let mut store = Store::open(Ok(path.clone()), Overrides::default());
        store.save(Settings::default()).unwrap();
        assert!(store.error.is_none());
        assert!(Store::open(Ok(path), Overrides::default()).online_lyrics());
    }

    #[cfg(unix)]
    #[test]
    fn dangling_settings_symlink_leaves_online_lyrics_disabled() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        std::os::unix::fs::symlink(directory.path().join("missing"), &path).unwrap();
        let store = Store::open(Ok(path), Overrides::default());
        assert!(!store.online_lyrics());
        assert!(store.error.is_some());
    }

    #[test]
    fn altgr_can_enter_printable_speaker_name_characters() {
        let key = KeyEvent::new(
            KeyCode::Char('ł'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert_eq!(Dialog::key(key), Some(Input::Character('ł')));
    }
}
