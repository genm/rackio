//! Versioned per-user desktop preferences.
//!
//! These are choices about this desktop app (notifications, appearance), never
//! daemon configuration, pairing material or metrics. They live in the
//! desktop's own config directory so the React window and the native runtime
//! can share one durable source instead of the renderer's localStorage.
//!
//! A missing file is the first-run state. An unreadable or corrupt file is
//! reported through `load_error` and treated as the defaults, which keep
//! notifications off: a damaged file never opts the user in.

use std::{
    io::Write as _,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

const PREFERENCES_FILE: &str = "desktop-preferences.json";
const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NotificationThreshold {
    Warning,
    Degraded,
    Critical,
    Offline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Appearance {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Preferences {
    pub version: u32,
    pub notifications_enabled: bool,
    pub notification_threshold: NotificationThreshold,
    pub appearance: Appearance,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: CURRENT_VERSION,
            notifications_enabled: false,
            notification_threshold: NotificationThreshold::Critical,
            appearance: Appearance::System,
        }
    }
}

/// What the renderer receives: the effective preferences plus whether they
/// came from a damaged file or a legacy migration still pending.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreferencesView {
    #[serde(flatten)]
    pub preferences: Preferences,
    /// Why the saved file could not be used, if it could not.
    pub load_error: Option<String>,
    /// False until preferences have been written once; the renderer migrates
    /// its legacy localStorage values only while this is false.
    pub persisted: bool,
}

pub(crate) struct DesktopPreferences {
    path: Option<PathBuf>,
    state: Mutex<Loaded>,
}

struct Loaded {
    preferences: Preferences,
    load_error: Option<String>,
    persisted: bool,
}

impl DesktopPreferences {
    pub(crate) fn load(config_dir: Option<PathBuf>) -> Self {
        let path = config_dir.map(|directory| directory.join(PREFERENCES_FILE));
        let state = match path.as_deref() {
            None => Loaded {
                preferences: Preferences::default(),
                load_error: Some(String::from("Rackio has no config directory.")),
                persisted: false,
            },
            Some(path) => match read(path) {
                Ok(Some(preferences)) => Loaded {
                    preferences,
                    load_error: None,
                    persisted: true,
                },
                Ok(None) => Loaded {
                    preferences: Preferences::default(),
                    load_error: None,
                    persisted: false,
                },
                Err(error) => {
                    tracing::error!(%error, "Ignoring unreadable desktop preferences");
                    Loaded {
                        preferences: Preferences::default(),
                        load_error: Some(error),
                        persisted: false,
                    }
                }
            },
        };
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    pub(crate) fn view(&self) -> Result<PreferencesView, String> {
        let state = self.lock()?;
        Ok(PreferencesView {
            preferences: state.preferences.clone(),
            load_error: state.load_error.clone(),
            persisted: state.persisted,
        })
    }

    /// Replace the preferences and persist them. The in-memory value changes
    /// only after the file is written, so a failed save never reports a
    /// preference that will not survive a restart.
    pub(crate) fn set(&self, next: Preferences) -> Result<PreferencesView, String> {
        if next.version != CURRENT_VERSION {
            return Err(format!("Unsupported preferences version {}.", next.version));
        }
        let path = self.path.as_deref().ok_or_else(|| {
            String::from("Rackio has no config directory to save preferences in.")
        })?;
        let mut state = self.lock()?;
        write(path, &next)?;
        *state = Loaded {
            preferences: next,
            load_error: None,
            persisted: true,
        };
        drop(state);
        self.view()
    }

    /// Carry the legacy localStorage choices over once. Idempotent: if
    /// preferences were already persisted, the stored values win and nothing
    /// is written, so a repeated or racing migration cannot undo a later change.
    pub(crate) fn migrate_legacy(
        &self,
        notifications_enabled: bool,
        threshold: NotificationThreshold,
    ) -> Result<PreferencesView, String> {
        if self.lock()?.persisted {
            return self.view();
        }
        let mut next = self.lock()?.preferences.clone();
        next.notifications_enabled = notifications_enabled;
        next.notification_threshold = threshold;
        self.set(next)
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Loaded>, String> {
        self.state
            .lock()
            .map_err(|_| String::from("Desktop preferences are unavailable."))
    }
}

fn read(path: &Path) -> Result<Option<Preferences>, String> {
    let contents = match std::fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    let preferences: Preferences = serde_json::from_slice(&contents)
        .map_err(|error| format!("{} is not valid: {error}", path.display()))?;
    if preferences.version != CURRENT_VERSION {
        return Err(format!(
            "{} has unsupported version {}",
            path.display(),
            preferences.version
        ));
    }
    Ok(Some(preferences))
}

/// Write through a temporary file in the same directory so a crash mid-save
/// leaves the previous preferences intact.
fn write(path: &Path, preferences: &Preferences) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| String::from("Preferences have no parent directory."))?;
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
    let contents = serde_json::to_vec_pretty(preferences)
        .map_err(|error| format!("Could not encode preferences: {error}"))?;
    let mut file = tempfile::NamedTempFile::new_in(directory)
        .map_err(|error| format!("Could not save preferences: {error}"))?;
    file.write_all(&contents)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|error| format!("Could not save preferences: {error}"))?;
    file.persist(path)
        .map(|_| ())
        .map_err(|error| format!("Could not save preferences: {error}"))
}

// Async so Tauri runs these off the main thread: a save syncs to disk.
#[tauri::command]
pub(crate) async fn get_desktop_preferences(
    store: tauri::State<'_, DesktopPreferences>,
) -> Result<PreferencesView, String> {
    store.view()
}

#[tauri::command]
pub(crate) async fn set_desktop_preferences(
    store: tauri::State<'_, DesktopPreferences>,
    preferences: Preferences,
) -> Result<PreferencesView, String> {
    store.set(preferences)
}

#[tauri::command]
pub(crate) async fn migrate_legacy_preferences(
    store: tauri::State<'_, DesktopPreferences>,
    notifications_enabled: bool,
    threshold: NotificationThreshold,
) -> Result<PreferencesView, String> {
    store.migrate_legacy(notifications_enabled, threshold)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, DesktopPreferences) {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let store = DesktopPreferences::load(Some(directory.path().to_owned()));
        (directory, store)
    }

    #[test]
    fn first_run_defaults_keep_notifications_off() {
        let (_directory, store) = store();
        let view = store.view().unwrap_or_else(|error| panic!("{error}"));
        assert!(!view.preferences.notifications_enabled);
        assert_eq!(view.preferences.appearance, Appearance::System);
        assert!(!view.persisted);
        assert!(view.load_error.is_none());
    }

    #[test]
    fn preferences_survive_a_restart() {
        let (directory, store) = store();
        let next = Preferences {
            notifications_enabled: true,
            notification_threshold: NotificationThreshold::Offline,
            appearance: Appearance::Dark,
            ..Preferences::default()
        };
        store
            .set(next.clone())
            .unwrap_or_else(|error| panic!("{error}"));
        let reloaded = DesktopPreferences::load(Some(directory.path().to_owned()));
        let view = reloaded.view().unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(view.preferences, next);
        assert!(view.persisted);
    }

    #[test]
    fn legacy_migration_preserves_choices_and_runs_once() {
        let (directory, store) = store();
        let view = store
            .migrate_legacy(true, NotificationThreshold::Warning)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(view.preferences.notifications_enabled);
        assert_eq!(
            view.preferences.notification_threshold,
            NotificationThreshold::Warning
        );

        // A later change must not be undone by a repeated migration.
        let mut changed = view.preferences.clone();
        changed.notifications_enabled = false;
        store.set(changed).unwrap_or_else(|error| panic!("{error}"));
        let again = store
            .migrate_legacy(true, NotificationThreshold::Offline)
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(!again.preferences.notifications_enabled);
        assert_eq!(
            again.preferences.notification_threshold,
            NotificationThreshold::Warning
        );
        drop(directory);
    }

    #[test]
    fn a_corrupt_file_is_reported_and_never_opts_the_user_in() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(directory.path().join(PREFERENCES_FILE), b"{not json")
            .unwrap_or_else(|error| panic!("{error}"));
        let view = DesktopPreferences::load(Some(directory.path().to_owned()))
            .view()
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(view.load_error.is_some());
        assert!(!view.preferences.notifications_enabled);
        assert!(!view.persisted);
    }

    #[test]
    fn an_unsupported_version_is_refused_on_load_and_on_save() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(
            directory.path().join(PREFERENCES_FILE),
            br#"{"version":99,"notificationsEnabled":true,"notificationThreshold":"warning","appearance":"dark"}"#,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let store = DesktopPreferences::load(Some(directory.path().to_owned()));
        let view = store.view().unwrap_or_else(|error| panic!("{error}"));
        assert!(view.load_error.is_some());
        assert!(!view.preferences.notifications_enabled);

        let future = Preferences {
            version: 2,
            ..Preferences::default()
        };
        assert!(store.set(future).is_err());
    }

    #[test]
    fn a_failed_save_is_reported_and_not_applied() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let blocked = directory.path().join("not-a-directory");
        std::fs::write(&blocked, b"").unwrap_or_else(|error| panic!("{error}"));
        let store = DesktopPreferences::load(Some(blocked));
        let next = Preferences {
            notifications_enabled: true,
            ..Preferences::default()
        };
        assert!(store.set(next).is_err());
        let view = store.view().unwrap_or_else(|error| panic!("{error}"));
        assert!(!view.preferences.notifications_enabled);
        assert!(!view.persisted);

        let unsaved = DesktopPreferences::load(None);
        assert!(unsaved.set(Preferences::default()).is_err());
    }

    #[test]
    fn unknown_stored_fields_are_not_trusted() {
        assert!(serde_json::from_str::<Preferences>(
            r#"{"version":1,"notificationsEnabled":false,"notificationThreshold":"critical","appearance":"system","extra":1}"#
        )
        .is_err());
    }
}
