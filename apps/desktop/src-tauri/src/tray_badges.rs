//! Per-machine menu-bar badges: the one glyph a machine's tray item shows.
//!
//! A machine's full name in the menu bar crowds out every other status item
//! once a rack has more than a couple of machines, so each tray item shows a
//! single glyph instead: the operator's chosen icon, or else the first letter
//! of the machine's name. The full name stays in the tooltip and the menu.
//!
//! The choice is a viewer preference of this desktop, not machine state, so it
//! lives in the desktop's own config directory rather than in the daemon or on
//! the wire. It is the single place the badge rule is implemented: the tray
//! and the dashboard both read the resolved `trayLabel` from the fleet
//! snapshot.

use std::{
    collections::BTreeMap,
    io::Write as _,
    path::{Path, PathBuf},
    sync::Mutex,
};
use unicode_segmentation::UnicodeSegmentation as _;

const BADGES_FILE: &str = "tray-badges.json";

pub(crate) struct TrayBadges {
    path: Option<PathBuf>,
    icons: Mutex<BTreeMap<String, String>>,
}

impl TrayBadges {
    /// Load the saved icons. A missing file is the normal first-run state; an
    /// unreadable or corrupt one is reported and ignored so the tray still
    /// shows initials, and the next save replaces it with a valid file.
    pub(crate) fn load(config_dir: Option<PathBuf>) -> Self {
        let path = config_dir.map(|directory| directory.join(BADGES_FILE));
        let icons = match path.as_deref().map(read_icons) {
            Some(Ok(icons)) => icons,
            Some(Err(error)) => {
                tracing::error!(%error, "Ignoring unreadable tray badge preferences");
                BTreeMap::new()
            }
            None => {
                tracing::error!("No config directory; tray icons cannot be saved");
                BTreeMap::new()
            }
        };
        Self {
            path,
            icons: Mutex::new(icons),
        }
    }

    fn icon(&self, machine_id: &str) -> Option<String> {
        self.icons
            .lock()
            .ok()
            .and_then(|icons| icons.get(machine_id).cloned())
    }

    /// Add the resolved `trayLabel`, and the custom `trayIcon` when one is set,
    /// to one machine of the fleet snapshot.
    pub(crate) fn decorate(&self, node: &mut serde_json::Value) {
        let Some(object) = node.as_object_mut() else {
            return;
        };
        let icon = object
            .get("id")
            .and_then(serde_json::Value::as_str)
            .and_then(|id| self.icon(id));
        let name = object
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let label = badge_label(icon.as_deref(), name);
        object.insert(String::from("trayLabel"), serde_json::Value::String(label));
        object.insert(
            String::from("trayIcon"),
            icon.map_or(serde_json::Value::Null, serde_json::Value::String),
        );
    }

    /// Set (`Some`) or clear (`None`) a machine's icon and persist the result.
    /// The in-memory preference only changes once the file is written, so a
    /// failed save never leaves the tray showing an icon that will not survive
    /// a restart.
    pub(crate) fn set(&self, machine_id: &str, icon: Option<&str>) -> Result<(), String> {
        if machine_id.is_empty() {
            return Err(String::from("A tray icon needs a machine to belong to."));
        }
        let icon = icon.map(validate_icon).transpose()?;
        let path = self
            .path
            .as_deref()
            .ok_or_else(|| String::from("Rackio has no config directory to save tray icons in."))?;
        let mut icons = self
            .icons
            .lock()
            .map_err(|_| String::from("Tray icon preferences are unavailable."))?;
        let mut next = icons.clone();
        match icon {
            Some(icon) => next.insert(machine_id.to_owned(), icon),
            None => next.remove(machine_id),
        };
        write_icons(path, &next)?;
        *icons = next;
        Ok(())
    }
}

/// The glyph a tray item shows: the custom icon, or the machine name's first
/// character in upper case (when that stays one character), or `?` for a
/// machine with no name at all.
pub(crate) fn badge_label(icon: Option<&str>, name: &str) -> String {
    if let Some(icon) = icon {
        return icon.to_owned();
    }
    let Some(first) = name.trim().graphemes(true).next() else {
        return String::from("?");
    };
    let upper = first.to_uppercase();
    if upper.graphemes(true).count() == 1 {
        upper
    } else {
        first.to_owned()
    }
}

/// An icon is exactly one visible character — a letter, a symbol, or one
/// emoji (including multi-code-point emoji such as flags or ZWJ sequences).
/// The single-glyph limit is the point of the feature: anything wider brings
/// back the menu-bar crowding the badge exists to remove.
fn validate_icon(icon: &str) -> Result<String, String> {
    let icon = icon.trim();
    let mut graphemes = icon.graphemes(true);
    match (graphemes.next(), graphemes.next()) {
        (Some(glyph), None) if !glyph.chars().any(|c| c.is_control() || c.is_whitespace()) => {
            Ok(glyph.to_owned())
        }
        _ => Err(String::from(
            "A tray icon must be a single character or emoji.",
        )),
    }
}

fn read_icons(path: &Path) -> Result<BTreeMap<String, String>, String> {
    let contents = match std::fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    let icons: BTreeMap<String, String> = serde_json::from_slice(&contents)
        .map_err(|error| format!("{} is not valid: {error}", path.display()))?;
    // A hand-edited entry that breaks the single-glyph rule is dropped rather
    // than rendered: it would widen the menu bar the rule exists to protect.
    Ok(icons
        .into_iter()
        .filter_map(|(id, icon)| validate_icon(&icon).ok().map(|icon| (id, icon)))
        .collect())
}

/// Write through a temporary file in the same directory, so a crash mid-save
/// leaves the previous preferences intact rather than a truncated file.
fn write_icons(path: &Path, icons: &BTreeMap<String, String>) -> Result<(), String> {
    let directory = path
        .parent()
        .ok_or_else(|| String::from("Tray icon preferences have no parent directory."))?;
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("Could not create {}: {error}", directory.display()))?;
    let contents = serde_json::to_vec_pretty(icons)
        .map_err(|error| format!("Could not encode tray icons: {error}"))?;
    let mut file = tempfile::NamedTempFile::new_in(directory)
        .map_err(|error| format!("Could not save tray icons: {error}"))?;
    file.write_all(&contents)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|error| format!("Could not save tray icons: {error}"))?;
    file.persist(path)
        .map(|_| ())
        .map_err(|error| format!("Could not save tray icons: {error}"))
}

// Async so Tauri runs it off the main thread: the save syncs to disk, and the
// main thread is the one that draws the status items.
#[tauri::command]
pub(crate) async fn set_tray_icon(
    badges: tauri::State<'_, TrayBadges>,
    machine_id: String,
    icon: Option<String>,
) -> Result<(), String> {
    badges.set(&machine_id, icon.as_deref())
}

#[cfg(test)]
mod tests {
    use super::{BADGES_FILE, TrayBadges, badge_label, validate_icon};

    #[test]
    fn a_machine_without_an_icon_shows_its_initial() {
        assert_eq!(badge_label(None, "steamdeck"), "S");
        assert_eq!(badge_label(None, "  mac mini"), "M");
        assert_eq!(badge_label(None, "開発機"), "開");
        // A letter whose upper case is two letters keeps its own form rather
        // than widening the badge.
        assert_eq!(badge_label(None, "ßeta"), "ß");
        assert_eq!(badge_label(None, ""), "?");
        assert_eq!(badge_label(Some("🎮"), "steamdeck"), "🎮");
    }

    #[test]
    fn an_icon_is_exactly_one_visible_glyph() {
        assert_eq!(validate_icon("🎮"), Ok(String::from("🎮")));
        assert_eq!(validate_icon(" S "), Ok(String::from("S")));
        // Multi-code-point emoji are still one glyph in the menu bar.
        assert_eq!(validate_icon("🇯🇵"), Ok(String::from("🇯🇵")));
        assert_eq!(validate_icon("👩‍💻"), Ok(String::from("👩‍💻")));
        assert!(validate_icon("").is_err());
        assert!(validate_icon("   ").is_err());
        assert!(validate_icon("Srv").is_err());
        assert!(validate_icon("🎮🎮").is_err());
        assert!(validate_icon("\u{7}").is_err());
    }

    #[test]
    fn icons_persist_across_restarts_and_can_be_cleared() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let config = directory.path().join("config");
        let badges = TrayBadges::load(Some(config.clone()));
        badges
            .set("machine-a", Some("🖥"))
            .unwrap_or_else(|error| panic!("{error}"));

        let reloaded = TrayBadges::load(Some(config.clone()));
        let mut node = serde_json::json!({ "id": "machine-a", "name": "Server" });
        reloaded.decorate(&mut node);
        assert_eq!(node["trayLabel"], "🖥");
        assert_eq!(node["trayIcon"], "🖥");

        reloaded
            .set("machine-a", None)
            .unwrap_or_else(|error| panic!("{error}"));
        let mut node = serde_json::json!({ "id": "machine-a", "name": "Server" });
        TrayBadges::load(Some(config)).decorate(&mut node);
        assert_eq!(node["trayLabel"], "S");
        assert!(node["trayIcon"].is_null());
    }

    #[test]
    fn a_rejected_icon_changes_nothing() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let badges = TrayBadges::load(Some(directory.path().to_owned()));
        badges
            .set("machine-a", Some("A"))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(badges.set("machine-a", Some("Server")).is_err());
        assert!(badges.set("", Some("B")).is_err());
        let mut node = serde_json::json!({ "id": "machine-a", "name": "Server" });
        badges.decorate(&mut node);
        assert_eq!(node["trayLabel"], "A");
    }

    #[test]
    fn a_failed_save_is_reported_and_not_applied() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        // A regular file where the config directory should be makes the save
        // fail the way a read-only or missing volume would.
        let blocked = directory.path().join("not-a-directory");
        std::fs::write(&blocked, b"").unwrap_or_else(|error| panic!("{error}"));
        let badges = TrayBadges::load(Some(blocked));
        assert!(badges.set("machine-a", Some("🎮")).is_err());
        let mut node = serde_json::json!({ "id": "machine-a", "name": "steamdeck" });
        badges.decorate(&mut node);
        assert_eq!(node["trayLabel"], "S");

        let unsaved = TrayBadges::load(None);
        assert!(unsaved.set("machine-a", Some("🎮")).is_err());
    }

    #[test]
    fn corrupt_or_invalid_saved_icons_fall_back_to_initials() {
        let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(directory.path().join(BADGES_FILE), b"{not json")
            .unwrap_or_else(|error| panic!("{error}"));
        let mut node = serde_json::json!({ "id": "machine-a", "name": "server" });
        TrayBadges::load(Some(directory.path().to_owned())).decorate(&mut node);
        assert_eq!(node["trayLabel"], "S");

        std::fs::write(
            directory.path().join(BADGES_FILE),
            br#"{"machine-a":"too wide","machine-b":"B"}"#,
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let badges = TrayBadges::load(Some(directory.path().to_owned()));
        let mut a = serde_json::json!({ "id": "machine-a", "name": "server" });
        let mut b = serde_json::json!({ "id": "machine-b", "name": "other" });
        badges.decorate(&mut a);
        badges.decorate(&mut b);
        assert_eq!(a["trayLabel"], "S");
        assert_eq!(b["trayLabel"], "B");
    }
}
