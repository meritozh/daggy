//! Favorite folders and the current-scan location for the sidebar.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub id: &'static str,
    pub name: String,
    pub path: PathBuf,
}

/// Locations that exist on this machine. `current` becomes Recents when it is
/// not already one of the named favorites.
pub fn sidebar_locations(current: Option<&Path>) -> Vec<Location> {
    collect_locations(favorite_candidates(current), None)
}

pub fn collect_locations(
    candidates: impl IntoIterator<Item = Location>,
    current: Option<&Path>,
) -> Vec<Location> {
    let mut locations: Vec<Location> = candidates
        .into_iter()
        .filter(|location| location.path.is_dir())
        .collect();
    if let Some(current) = current {
        if current.is_dir()
            && !locations
                .iter()
                .any(|location| same_path(&location.path, current))
        {
            let name = current
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| current.to_string_lossy().into_owned());
            locations.push(Location {
                id: "current",
                name,
                path: current.to_path_buf(),
            });
        }
    }
    locations
}

pub fn location_is_active(location: &Path, current: Option<&Path>) -> bool {
    current.is_some_and(|path| same_path(location, path))
}

/// Last path component for window titles and toolbar labels. The HIG titles a
/// document window with the document name, not the full filesystem path.
pub fn display_name(path: &Path) -> String {
    if path.as_os_str() == "/" {
        return "Macintosh HD".into();
    }
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

pub fn window_title(path: Option<&Path>) -> String {
    path.map(display_name).unwrap_or_else(|| "Daggy".into())
}

pub fn title_bar_label(path: Option<&Path>, used: Option<(u64, u64)>) -> String {
    let name = window_title(path);
    match used {
        Some((used, total)) if total > 0 => format!(
            "{name} — {} of {} used",
            daggy_core::fmt::human_bytes(used),
            daggy_core::fmt::human_bytes(total)
        ),
        _ => name,
    }
}

fn favorite_candidates(current: Option<&Path>) -> Vec<Location> {
    let mut candidates = Vec::new();
    let named = named_favorites();
    if let Some(current) = current {
        if current.is_dir()
            && current.as_os_str() != "/"
            && !named
                .iter()
                .any(|location| same_path(&location.path, current))
        {
            candidates.push(Location {
                id: "recents",
                name: "Recents".into(),
                path: current.to_path_buf(),
            });
        }
    }
    candidates.extend(named);
    candidates
}

fn named_favorites() -> Vec<Location> {
    let mut candidates = Vec::new();
    push_candidate(
        &mut candidates,
        "downloads",
        "Downloads",
        dirs::download_dir(),
    );
    push_candidate(
        &mut candidates,
        "applications",
        "Applications",
        Some(PathBuf::from("/Applications")),
    );
    push_candidate(&mut candidates, "photos", "Photos", dirs::picture_dir());
    push_candidate(&mut candidates, "movies", "Movies", dirs::video_dir());
    push_candidate(&mut candidates, "home", "Home Folder", dirs::home_dir());
    candidates
}

fn push_candidate(into: &mut Vec<Location>, id: &'static str, name: &str, path: Option<PathBuf>) {
    if let Some(path) = path {
        into.push(Location {
            id,
            name: name.to_string(),
            path,
        });
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    left == right
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("daggy-locations-{}-{}", std::process::id(), stamp));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn collect_locations_keeps_existing_paths_and_appends_current() {
        let root = scratch();
        let home = root.join("Home");
        let docs = root.join("Documents");
        let other = root.join("Projects");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&docs).unwrap();
        fs::create_dir_all(&other).unwrap();
        let missing = root.join("Missing");

        let locations = collect_locations(
            [
                Location {
                    id: "home",
                    name: "Home".into(),
                    path: home.clone(),
                },
                Location {
                    id: "documents",
                    name: "Documents".into(),
                    path: docs.clone(),
                },
                Location {
                    id: "missing",
                    name: "Missing".into(),
                    path: missing,
                },
            ],
            Some(&other),
        );

        assert_eq!(
            locations
                .iter()
                .map(|location| location.id)
                .collect::<Vec<_>>(),
            ["home", "documents", "current"]
        );
        assert_eq!(locations[2].name, "Projects");
        assert!(location_is_active(&other, Some(&other)));
        assert!(!location_is_active(&home, Some(&other)));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn collect_locations_does_not_duplicate_current_favorite() {
        let root = scratch();
        fs::create_dir_all(&root).unwrap();
        let locations = collect_locations(
            [Location {
                id: "home",
                name: "Home".into(),
                path: root.clone(),
            }],
            Some(&root),
        );
        assert_eq!(locations.len(), 1);
        assert_eq!(locations[0].id, "home");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn window_title_uses_the_last_path_component() {
        assert_eq!(window_title(None), "Daggy");
        assert_eq!(
            window_title(Some(Path::new("/Users/ada/Documents"))),
            "Documents"
        );
        assert_eq!(window_title(Some(Path::new("/"))), "Macintosh HD");
    }

    #[test]
    fn title_bar_label_includes_capacity_when_known() {
        assert_eq!(
            title_bar_label(
                Some(Path::new("/")),
                Some((486 * 1024 * 1024 * 1024, 512 * 1024 * 1024 * 1024))
            ),
            "Macintosh HD — 486 GB of 512 GB used"
        );
        assert_eq!(title_bar_label(None, None), "Daggy");
    }

    #[test]
    fn favorite_candidates_use_recents_for_custom_folders() {
        let root = scratch();
        fs::create_dir_all(&root).unwrap();
        let ids: Vec<&str> = favorite_candidates(Some(&root))
            .iter()
            .map(|location| location.id)
            .collect();
        assert_eq!(ids.first().copied(), Some("recents"));
        assert!(ids.contains(&"home"));
        assert!(!ids.contains(&"desktop"));
        assert!(!ids.contains(&"documents"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn favorite_candidates_skip_recents_for_the_boot_volume() {
        let ids: Vec<&str> = favorite_candidates(Some(Path::new("/")))
            .iter()
            .map(|location| location.id)
            .collect();
        assert!(!ids.contains(&"recents"));
    }
}
