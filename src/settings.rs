//! 設定の既定値・正規化・JSON永続化。UIの下書きと適用はViewerが管理する。
//! 不足フィールドは既定値で補い、保存は一時ファイルからのrenameで行う。

use crate::i18n::{self, tr};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryAction {
    #[default]
    Ask,
    Wrap,
    Folder,
    Stop,
}
impl BoundaryAction {
    pub fn label(self) -> String {
        match self {
            Self::Ask => tr!("boundary-ask"),
            Self::Wrap => tr!("boundary-wrap"),
            Self::Folder => tr!("boundary-folder"),
            Self::Stop => tr!("boundary-stop"),
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Ask => Self::Wrap,
            Self::Wrap => Self::Folder,
            Self::Folder => Self::Stop,
            Self::Stop => Self::Ask,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortOrder {
    #[default]
    Name,
    Modified,
    Size,
    Extension,
    Random,
}
impl SortOrder {
    pub const ALL: [Self; 5] = [
        Self::Name,
        Self::Modified,
        Self::Size,
        Self::Extension,
        Self::Random,
    ];
    pub fn label(self) -> String {
        match self {
            Self::Name => tr!("name"),
            Self::Modified => tr!("modified"),
            Self::Size => tr!("file-size"),
            Self::Extension => tr!("extension"),
            Self::Random => tr!("random"),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub language: i18n::Language,
    pub shortcuts: std::collections::BTreeMap<String, crate::bindings::Shortcut>,
    pub boundary: BoundaryAction,
    pub sort: SortOrder,
    pub reverse: bool,
    pub natural_sort: bool,
    pub slideshow_seconds: u32,
    pub skip_count: u32,
    pub previous_folder_first: bool,
    pub parent_folder_first: bool,
    pub fit_on_open: bool,
    pub fit_upscale: bool,
    pub shrink_algorithm: crate::resample::Algorithm,
    pub enlarge_algorithm: crate::resample::Algorithm,
    pub lock_zoom: bool,
    pub prefetch: bool,
    pub background: u32,
    pub wheel_zoom: bool,
    pub manga: bool,
    pub right_to_left: bool,
    pub landscape_single: bool,
    pub manga_reverse_controls: bool,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            language: i18n::Language::default(),
            shortcuts: crate::bindings::defaults(),
            boundary: BoundaryAction::Ask,
            sort: SortOrder::Name,
            reverse: false,
            natural_sort: true,
            slideshow_seconds: 3,
            skip_count: 10,
            previous_folder_first: false,
            parent_folder_first: false,
            fit_on_open: true,
            fit_upscale: false,
            shrink_algorithm: crate::resample::Algorithm::default(),
            enlarge_algorithm: crate::resample::Algorithm::default(),
            lock_zoom: false,
            prefetch: true,
            background: 0x181a1f,
            wheel_zoom: false,
            manga: false,
            right_to_left: true,
            landscape_single: true,
            manga_reverse_controls: true,
        }
    }
}
impl Settings {
    pub fn normalize(&mut self) {
        self.slideshow_seconds = self.slideshow_seconds.clamp(1, 3600);
        self.skip_count = self.skip_count.clamp(1, 10000);
        self.background &= 0xffffff;
    }
    pub fn path() -> Result<PathBuf, String> {
        if let Some(path) = std::env::var_os("XDG_CONFIG_HOME").filter(|p| !p.is_empty()) {
            return Ok(PathBuf::from(path).join("nagame/settings.json"));
        }
        #[cfg(target_os = "windows")]
        if let Some(path) = std::env::var_os("APPDATA") {
            return Ok(PathBuf::from(path).join("nagame/settings.json"));
        }
        std::env::var_os("HOME")
            .filter(|p| !p.is_empty())
            .map(|p| PathBuf::from(p).join(".config/nagame/settings.json"))
            .ok_or(tr!("settings-path-missing"))
    }
    pub fn read(path: &Path) -> Result<Self, String> {
        match fs::read(path) {
            Ok(bytes) => {
                let mut settings: Self = serde_json::from_slice(&bytes)
                    .map_err(|e| i18n::settings_read_error(&e.to_string()))?;
                settings.normalize();
                Ok(settings)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let parent = path.parent().ok_or(tr!("settings-path-invalid"))?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let temporary = parent.join(format!(".settings-{}-{stamp}.tmp", std::process::id()));
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let mut normalized = self.clone();
            normalized.normalize();
            file.write_all(&serde_json::to_vec_pretty(&normalized)?)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, path)?;
            Ok::<_, std::io::Error>(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result.map_err(|e| i18n::settings_save_error(&e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_survive_restart_and_old_files_get_defaults() {
        let directory =
            std::env::temp_dir().join(format!("nagame-settings-{}", std::process::id()));
        let path = directory.join("nested/settings.json");
        assert_eq!(Settings::read(&path).unwrap(), Settings::default());
        let saved = Settings {
            language: i18n::Language::English,
            slideshow_seconds: 17,
            shrink_algorithm: crate::resample::Algorithm::Lanczos3,
            enlarge_algorithm: crate::resample::Algorithm::Nearest,
            boundary: BoundaryAction::Folder,
            reverse: true,
            ..Settings::default()
        };
        saved.save(&path).unwrap();
        assert_eq!(Settings::read(&path).unwrap(), saved);
        fs::write(&path, br#"{"slideshow_seconds":0,"sort":"size"}"#).unwrap();
        let old = Settings::read(&path).unwrap();
        assert_eq!(old.language, i18n::Language::System);
        assert_eq!(old.slideshow_seconds, 1);
        assert_eq!(old.sort, SortOrder::Size);
        assert!(old.prefetch);
        fs::write(&path, b"broken JSON").unwrap();
        assert!(Settings::read(&path).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"broken JSON");
        fs::remove_dir_all(directory).unwrap();
    }
}
