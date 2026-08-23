//! Mount-point capacity for the Locations card and window title.

use std::path::{Path, PathBuf};

use crate::locations::display_name;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeInfo {
    pub name: String,
    pub mount: PathBuf,
    pub total: u64,
    pub free: u64,
}

impl VolumeInfo {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }

    pub fn share(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.used() as f32 / self.total as f32
        }
    }

    pub fn is_mount(&self, path: Option<&Path>) -> bool {
        path.is_some_and(|path| path == self.mount)
    }

    pub fn title(&self) -> String {
        crate::locations::title_bar_label(
            Some(Path::new(&self.name)),
            Some((self.used(), self.total)),
        )
    }
}

/// Capacity of the volume that contains `path`. `None` when the kernel
/// cannot describe the mount.
pub fn volume_info(path: &Path) -> Option<VolumeInfo> {
    volume_info_at(path)
}

#[cfg(target_os = "macos")]
fn volume_info_at(path: &Path) -> Option<VolumeInfo> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;

    let mut bytes = path.as_os_str().as_bytes().to_vec();
    bytes.push(0);
    let mut buf = std::mem::MaybeUninit::<libc::statfs>::uninit();
    let result = unsafe { libc::statfs(bytes.as_ptr().cast(), buf.as_mut_ptr()) };
    if result != 0 {
        return None;
    }
    let buf = unsafe { buf.assume_init() };
    let block = buf.f_bsize as u64;
    let total = buf.f_blocks.saturating_mul(block);
    let free = buf.f_bavail.saturating_mul(block);
    if total == 0 {
        return None;
    }
    let mount_bytes = unsafe { CStr::from_ptr(buf.f_mntonname.as_ptr()) }.to_bytes();
    let mount = PathBuf::from(os_from_bytes(mount_bytes));
    Some(VolumeInfo {
        name: visible_volume_name(&mount),
        mount,
        total,
        free,
    })
}

fn visible_volume_name(mount: &Path) -> String {
    if let Some(name) = volumes_label(mount) {
        return name;
    }
    if mount == Path::new("/") || mount == Path::new("/System/Volumes/Data") {
        return "Macintosh HD".into();
    }
    display_name(mount)
}

fn volumes_label(mount: &Path) -> Option<String> {
    let mount_canon = std::fs::canonicalize(mount).unwrap_or_else(|_| mount.to_path_buf());
    let entries = std::fs::read_dir("/Volumes").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let target = std::fs::canonicalize(&path).unwrap_or(path);
        let boot_data = mount == Path::new("/System/Volumes/Data")
            && (target == Path::new("/") || name == "Macintosh HD");
        if target == mount_canon || boot_data {
            let label = name.to_string_lossy().into_owned();
            if !label.is_empty() && label != "Data" {
                return Some(label);
            }
            if boot_data {
                return Some("Macintosh HD".into());
            }
        }
    }
    None
}

#[cfg(target_os = "macos")]
fn os_from_bytes(bytes: &[u8]) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec(bytes.to_vec())
}

#[cfg(not(target_os = "macos"))]
fn volume_info_at(_path: &Path) -> Option<VolumeInfo> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn used_is_total_minus_free() {
        let volume = VolumeInfo {
            name: "Macintosh HD".into(),
            mount: PathBuf::from("/"),
            total: 512,
            free: 26,
        };
        assert_eq!(volume.used(), 486);
        assert!((volume.share() - 486.0 / 512.0).abs() < f32::EPSILON);
        assert!(volume.title().starts_with("Macintosh HD — "));
        assert!(volume.is_mount(Some(Path::new("/"))));
        assert!(!volume.is_mount(Some(Path::new("/Users"))));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn root_volume_reports_capacity() {
        let volume = volume_info(Path::new("/")).expect("statfs on /");
        assert!(volume.total > 0);
        assert!(volume.used() <= volume.total);
        assert!(
            volume.mount == PathBuf::from("/")
                || volume.mount == PathBuf::from("/System/Volumes/Data")
        );
        assert!(!volume.name.is_empty());
    }
}
