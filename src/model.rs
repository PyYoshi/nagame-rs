//! 画像一覧・並び順・フォルダ探索と、表示倍率・パンの計算。
//! デコードやWindow操作を持たず、AlbumとViewportの状態を扱う。

use crate::i18n::tr;
use crate::settings::{Settings, SortOrder};
use std::{
    cmp::Ordering,
    fs,
    path::{Path, PathBuf},
};

/// 拡張子から、一覧に含める対応形式か判定する。
pub fn supported(path: &Path) -> bool {
    path.extension().and_then(|s| s.to_str()).is_some_and(|s| {
        matches!(
            s.to_ascii_lowercase().as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "jpe"
                | "gif"
                | "bmp"
                | "webp"
                | "tif"
                | "tiff"
                | "ico"
                | "avif"
                | "heif"
                | "heic"
                | "hif"
        )
    })
}

/// 自然順の比較。大文字小文字を区別せず、数字の並びは先頭の0を除いた桁数と文字で比べる
/// （整数に変換しないので桁あふれしない）。同順なら元の文字列で決める。
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    let (a_bytes, b_bytes) = (a_lower.as_bytes(), b_lower.as_bytes());
    let (mut i, mut j) = (0, 0);
    while i < a_bytes.len() && j < b_bytes.len() {
        if a_bytes[i].is_ascii_digit() && b_bytes[j].is_ascii_digit() {
            let (start_i, start_j) = (i, j);
            while i < a_bytes.len() && a_bytes[i].is_ascii_digit() {
                i += 1;
            }
            while j < b_bytes.len() && b_bytes[j].is_ascii_digit() {
                j += 1;
            }
            let x = a_lower[start_i..i].trim_start_matches('0');
            let y = b_lower[start_j..j].trim_start_matches('0');
            let order = x.len().cmp(&y.len()).then_with(|| x.cmp(y));
            if order != Ordering::Equal {
                return order;
            }
        } else {
            let order = a_bytes[i].cmp(&b_bytes[j]);
            if order != Ordering::Equal {
                return order;
            }
            i += 1;
            j += 1;
        }
    }
    (a_bytes.len() - i)
        .cmp(&(b_bytes.len() - j))
        .then_with(|| a.cmp(b))
}

/// 1つのフォルダにある対応画像の一覧と、表示中の位置。
#[derive(Default, Clone)]
pub struct Album {
    pub directory: PathBuf,
    pub paths: Vec<PathBuf>,
    pub index: usize,
}

impl Album {
    /// ファイルならそのフォルダを開いてそのファイルを選び、フォルダなら先頭を選ぶ。並びは自然順。
    pub fn open(path: &Path) -> Result<Self, String> {
        let path = path.canonicalize().map_err(|e| e.to_string())?;
        if !path.is_dir() && !supported(&path) {
            return Err(tr!("supported-formats"));
        }
        let directory = if path.is_dir() {
            path.as_path()
        } else {
            path.parent().ok_or(tr!("parent-missing"))?
        };
        let mut paths = Vec::new();
        for entry in fs::read_dir(directory).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let candidate = entry.path();
            if candidate.is_file() && supported(&candidate) {
                paths.push(candidate);
            }
        }
        paths.sort_by(|a, b| {
            natural_cmp(
                &a.file_name().unwrap().to_string_lossy(),
                &b.file_name().unwrap().to_string_lossy(),
            )
            .then_with(|| a.cmp(b))
        });

        let index = paths.iter().position(|p| p == &path).unwrap_or(0);
        Ok(Self {
            paths,
            index,
            directory: directory.to_path_buf(),
        })
    }

    /// 設定の並び順に並べ替える。表示中の画像は選んだままにする。
    pub fn sort(&mut self, settings: &Settings) {
        let selected = self.current().map(PathBuf::from);
        let mut items: Vec<_> = std::mem::take(&mut self.paths)
            .into_iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let extension = path
                    .extension()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_lowercase();
                let metadata = if matches!(settings.sort, SortOrder::Modified | SortOrder::Size) {
                    fs::metadata(&path).ok()
                } else {
                    None
                };
                let modified = metadata.as_ref().and_then(|m| m.modified().ok());
                let size = metadata.map(|m| m.len()).unwrap_or(0);
                (path, name, extension, modified, size)
            })
            .collect();
        items.sort_by(|a, b| {
            let names = if settings.natural_sort {
                natural_cmp(&a.1, &b.1)
            } else {
                a.1.to_lowercase().cmp(&b.1.to_lowercase())
            };
            let order = match settings.sort {
                SortOrder::Modified => a.3.cmp(&b.3),
                SortOrder::Size => a.4.cmp(&b.4),
                SortOrder::Extension => a.2.cmp(&b.2),
                _ => Ordering::Equal,
            };
            order.then(names).then_with(|| a.0.cmp(&b.0))
        });
        if settings.sort == SortOrder::Random {
            let mut seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64
                | 1;
            for i in (1..items.len()).rev() {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                items.swap(i, seed as usize % (i + 1));
            }
        }
        if settings.reverse {
            items.reverse();
        }
        self.paths = items.into_iter().map(|item| item.0).collect();
        self.index = selected
            .and_then(|path| self.paths.iter().position(|p| p == &path))
            .unwrap_or(0);
    }

    /// 先読みする次と前の画像。端では反対側へ回り込む。
    pub fn neighbours(&self) -> Vec<PathBuf> {
        if self.paths.len() < 2 {
            return vec![];
        }
        [1isize, -1]
            .into_iter()
            .map(|step| {
                self.paths
                    [(self.index as isize + step).rem_euclid(self.paths.len() as isize) as usize]
                    .clone()
            })
            .collect()
    }

    /// 前後のフォルダを開く。adjacentなら同じ階層の隣のフォルダ（画像がなくてもよい）、
    /// そうでなければサブフォルダを含む深さ優先の順で、次に画像があるフォルダ。
    /// 並べ替えと選択位置は呼び出し側が決め直す。
    pub fn folder(&self, direction: isize, adjacent: bool) -> Result<Self, String> {
        let mut directory = self.directory.clone();
        for _ in 0..4096 {
            directory = next_directory(&directory, direction, adjacent)
                .ok_or(tr!("destination-missing"))?;
            if let Ok(mut album) = Self::open(&directory)
                && (adjacent || !album.paths.is_empty())
            {
                if direction < 0 {
                    album.index = album.paths.len().saturating_sub(1);
                }
                return Ok(album);
            }
        }
        Err(tr!("search-limit"))
    }

    pub fn parent(&self) -> Result<Self, String> {
        let parent = self.directory.parent().ok_or(tr!("parent-missing"))?;
        let mut album = Self::open(parent)?;
        album.index = album.paths.len().saturating_sub(1);
        Ok(album)
    }

    pub fn current(&self) -> Option<&Path> {
        self.paths.get(self.index).map(PathBuf::as_path)
    }

    pub fn step(&mut self, delta: isize) {
        if !self.paths.is_empty() {
            self.index =
                (self.index as isize + delta).rem_euclid(self.paths.len() as isize) as usize;
        }
    }
}

fn directories(parent: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(parent)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|entry| entry.path())
        .collect();
    paths.sort_by(|a, b| {
        natural_cmp(
            &a.file_name().unwrap().to_string_lossy(),
            &b.file_name().unwrap().to_string_lossy(),
        )
        .then_with(|| a.cmp(b))
    });
    paths
}

fn next_directory(directory: &Path, direction: isize, adjacent: bool) -> Option<PathBuf> {
    if direction > 0
        && !adjacent
        && let Some(child) = directories(directory).into_iter().next()
    {
        return Some(child);
    }
    let mut cursor = directory.to_path_buf();
    loop {
        let parent = cursor.parent()?;
        let siblings = directories(parent);
        let index = siblings.iter().position(|p| p == &cursor)?;
        let next = index as isize + direction;
        if next >= 0 && next < (siblings.len() as isize) {
            let mut target = siblings[next as usize].clone();
            if direction < 0 && !adjacent {
                while let Some(child) = directories(&target).into_iter().last() {
                    target = child;
                }
            }
            return Some(target);
        }
        if adjacent {
            return None;
        }
        if direction < 0 {
            return Some(parent.to_path_buf());
        }
        cursor = parent.to_path_buf();
    }
}

/// 表示倍率とパン。fitならウィンドウに合わせ、そうでなければzoomを使う。
#[derive(Clone, Copy, Debug)]
pub struct Viewport {
    pub fit: bool,
    pub zoom: f32,
    /// fitのとき、元の大きさより拡大するか。
    pub fit_upscale: bool,
    /// 画面中央からの画像のずれ（論理ピクセル）。
    pub pan: [f32; 2],
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            fit: true,
            zoom: 1.0,
            fit_upscale: false,
            pan: [0.0; 2],
        }
    }
}

impl Viewport {
    /// 表示倍率。fitならareaに収まる倍率（fit_upscaleでなければ等倍まで）。
    pub fn scale(&self, image: [f32; 2], area: [f32; 2]) -> f32 {
        if self.fit {
            (area[0] / image[0].max(1.0))
                .min(area[1] / image[1].max(1.0))
                .clamp(0.001, if self.fit_upscale { 32.0 } else { 1.0 })
        } else {
            self.zoom
        }
    }

    /// 現在の倍率にfactorを掛けて手動倍率にする（0.01〜32倍）。画面中央の位置を保つ。
    pub fn zoom_by(&mut self, factor: f32, image: [f32; 2], area: [f32; 2]) {
        let old = self.scale(image, area);
        self.zoom = (old * factor).clamp(0.01, 32.0);
        self.fit = false;
        for p in &mut self.pan {
            *p *= self.zoom / old;
        }
        self.clamp(image, area);
    }

    /// 画像が画面からはみ出す分だけパンできるよう制限する。収まる向きは中央に置く。
    pub fn clamp(&mut self, image: [f32; 2], area: [f32; 2]) {
        let scale = self.scale(image, area);
        for i in 0..2 {
            let limit = ((image[i] * scale - area[i]) / 2.0).max(0.0);
            self.pan[i] = self.pan[i].clamp(-limit, limit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_handles_large_numbers_and_japanese() {
        let mut names = [
            "画像10.PNG",
            "画像2.png",
            "画像1.png",
            "画像9999999999999999999999.png",
        ];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names[0], "画像1.png");
        assert_eq!(names[1], "画像2.png");
        assert_eq!(names[2], "画像10.PNG");
        assert_ne!(natural_cmp("a01.png", "a1.png"), Ordering::Equal);
    }

    #[test]
    fn navigation_wraps_both_ends_and_empty_is_safe() {
        let mut album = Album {
            directory: PathBuf::new(),
            paths: vec!["1.png".into(), "2.png".into()],
            index: 0,
        };
        album.step(-1);
        assert_eq!(album.index, 1);
        album.step(1);
        assert_eq!(album.index, 0);
        Album::default().step(-1);
    }

    #[test]
    fn fit_does_not_enlarge_and_pan_stays_in_bounds() {
        let mut view = Viewport::default();
        assert_eq!(view.scale([100., 50.], [800., 600.]), 1.);
        assert_eq!(view.scale([1600., 800.], [800., 600.]), 0.5);
        view.zoom_by(4., [1600., 800.], [800., 600.]);
        view.pan = [9999., -9999.];
        view.clamp([1600., 800.], [800., 600.]);
        assert_eq!(view.pan, [1200., -500.]);
        view.zoom_by(0.00001, [1600., 800.], [800., 600.]);
        assert_eq!(view.pan, [0., 0.]);
        assert_eq!(view.zoom, 0.01);
    }

    #[test]
    fn album_filters_directories_and_preserves_selected_file() {
        let root = std::env::temp_dir().join(format!("nagame-album-{}", std::process::id()));
        fs::create_dir_all(root.join("folder.png")).unwrap();
        for name in ["10.png", "2.PNG", "20.JPE", "notes.txt"] {
            fs::write(root.join(name), []).unwrap();
        }
        let album = Album::open(&root.join("10.png")).unwrap();
        assert_eq!(album.paths.len(), 3);
        assert_eq!(album.index, 1);
        assert_eq!(Album::open(&root.join("20.JPE")).unwrap().index, 2);
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod folder_tests {
    use super::*;
    #[test]
    fn folder_navigation_distinguishes_tree_siblings_and_parent() {
        let root = std::env::temp_dir().join(format!("nagame-folders-{}", std::process::id()));
        for name in ["01/child", "02-empty", "03"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        for name in ["01/1.png", "01/2.png", "01/child/1.avif", "03/1.heic"] {
            fs::write(root.join(name), []).unwrap();
        }
        let album = Album::open(&root.join("01")).unwrap();
        let child = album.folder(1, false).unwrap();
        assert!(child.directory.ends_with("01/child"));
        let sibling = album.folder(1, true).unwrap();
        assert!(sibling.directory.ends_with("02-empty"));
        assert!(sibling.paths.is_empty());
        let next = child.folder(1, false).unwrap();
        assert!(next.directory.ends_with("03"));
        let previous = next.folder(-1, false).unwrap();
        assert_eq!(previous.directory, child.directory);
        let parent = child.parent().unwrap();
        assert_eq!(parent.index, 1);
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod sort_tests {
    use super::*;
    #[test]
    fn sorting_preserves_selection_and_supports_size_reverse_and_lexical_names() {
        let root = std::env::temp_dir().join(format!("nagame-sort-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        for (name, size) in [("1.png", 3), ("2.png", 1), ("10.png", 2)] {
            fs::write(root.join(name), vec![0; size]).unwrap();
        }
        let mut album = Album::open(&root.join("2.png")).unwrap();
        album.sort(&Settings {
            sort: SortOrder::Size,
            ..Settings::default()
        });
        assert!(album.paths[0].ends_with("2.png"));
        assert!(album.paths[2].ends_with("1.png"));
        assert_eq!(album.index, 0);
        album.sort(&Settings {
            natural_sort: false,
            ..Settings::default()
        });
        assert!(album.paths[1].ends_with("10.png"));
        assert_eq!(album.index, 2);
        album.sort(&Settings {
            reverse: true,
            ..Settings::default()
        });
        assert!(album.paths[0].ends_with("10.png"));
        assert!(album.current().unwrap().ends_with("2.png"));
        fs::remove_dir_all(root).unwrap();
    }
}
