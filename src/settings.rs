use crate::{receiver_name, state::Visualizer};
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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct Settings {
    pub name: String,
    pub online_lyrics: bool,
    pub view: View,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            name: "t-ui-uxplay".into(),
            online_lyrics: true,
            view: View::Cover,
        }
    }
}

impl Settings {
    pub fn validated(mut self) -> Result<Self> {
        self.name = receiver_name(&self.name).map_err(anyhow::Error::msg)?;
        Ok(self)
    }
}

#[derive(Clone, Default)]
pub struct Overrides {
    pub name: Option<String>,
    pub no_online_lyrics: bool,
}

impl Overrides {
    fn name<'a>(&'a self, settings: &'a Settings) -> &'a str {
        self.name.as_deref().unwrap_or(&settings.name)
    }

    fn online_lyrics(&self, settings: &Settings) -> bool {
        !self.no_online_lyrics && settings.online_lyrics
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
    pub name_selected: bool,
    overrides: Overrides,
}

impl Dialog {
    pub fn new(store: &Store) -> Self {
        let mut dialog = Self {
            draft: store.saved.clone(),
            focus: Field::Name,
            mode: Mode::Edit,
            error: store.error.clone(),
            name_selected: true,
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

    pub fn locked(&self, field: Field) -> bool {
        match field {
            Field::Name => self.overrides.name.is_some(),
            Field::Lyrics => self.overrides.no_online_lyrics,
            Field::View | Field::Save => false,
        }
    }

    fn fields(&self) -> impl Iterator<Item = Field> + '_ {
        [Field::Name, Field::Lyrics, Field::View, Field::Save]
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
                        self.name_selected = self.focus == Field::Name;
                    }
                    Input::Enter => return self.activate(self.focus),
                    Input::Activate(field) => return self.activate(field),
                    Input::Character(c) if !c.is_control() => self.insert_name(&c.to_string()),
                    Input::Backspace if self.focus == Field::Name => {
                        if self.name_selected {
                            self.draft.name.clear();
                        } else if let Some((index, _)) =
                            self.draft.name.grapheme_indices(true).next_back()
                        {
                            self.draft.name.truncate(index);
                        }
                        self.name_selected = false;
                    }
                    _ => {}
                }
                None
            }
        }
    }

    pub fn insert_name(&mut self, text: &str) {
        if self.mode != Mode::Edit || self.focus != Field::Name || self.locked(Field::Name) {
            return;
        }
        if text.chars().any(char::is_control) {
            self.error = Some("Speaker names cannot contain control characters".into());
        } else {
            if self.name_selected {
                self.draft.name.clear();
                self.name_selected = false;
            }
            self.draft.name.push_str(text);
        }
    }

    fn activate(&mut self, field: Field) -> Option<Intent> {
        if self.locked(field) {
            return None;
        }
        self.focus = field;
        match field {
            Field::Name => self.name_selected = true,
            Field::Lyrics => self.draft.online_lyrics = !self.draft.online_lyrics,
            Field::View => self.draft.view = self.draft.view.next(),
            Field::Save => return Some(Intent::Save),
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            })
            .unwrap();
        let mut store = Store::open(
            Ok(path.clone()),
            Overrides {
                name: Some("CLI Speaker".into()),
                no_online_lyrics: true,
            },
        );
        assert_eq!(store.name(), "CLI Speaker");
        assert!(!store.online_lyrics());
        let mut dialog = Dialog::new(&store);
        assert_eq!(dialog.focus, Field::View);
        assert_eq!(dialog.name(), "CLI Speaker");
        assert!(!dialog.online_lyrics());
        dialog.edit(Input::Activate(Field::Lyrics));
        dialog.edit(Input::Activate(Field::View));
        store.save(dialog.draft).unwrap();

        let store = Store::open(Ok(path), Overrides::default());
        assert_eq!(store.name(), "ห้อง 音楽");
        assert!(store.online_lyrics());
        assert_eq!(store.saved.view, View::Spectrum);
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
