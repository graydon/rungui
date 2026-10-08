//! File-system model for the file manager example: directory listing, sorting, formatting,
//! copy / move / delete, and file previews. Pure std, no GUI types except `rungui::ImageData`,
//! so everything here is unit-tested under `cargo test`.
#![allow(dead_code)]

use rungui::ImageData;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Directories with more entries than this are cut off (the UI says so in the status bar).
pub const MAX_ENTRIES: usize = 50_000;
/// How much of a text file the preview reads.
pub const PREVIEW_LIMIT: usize = 64 * 1024;
/// How many bytes of a binary file the hex dump shows.
pub const HEX_LIMIT: usize = 4096;
/// Largest image file / edge length the preview will decode.
pub const IMAGE_MAX_BYTES: u64 = 4 * 1024 * 1024;
pub const IMAGE_MAX_EDGE: u32 = 1024;

#[derive(Clone, Debug)]
pub struct Entry {
    /// Lossy display name (non-UTF-8 bytes become U+FFFD). The real name is in `path`.
    pub name: String,
    pub path: PathBuf,
    /// Follows symlinks: a link to a directory is a directory.
    pub is_dir: bool,
    pub is_link: bool,
    /// A symlink whose target cannot be read.
    pub broken: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    /// The synthetic ".." row.
    pub is_parent: bool,
}

impl Entry {
    pub fn parent_of(dir: &Path) -> Option<Entry> {
        let p = dir.parent()?;
        Some(Entry {
            name: "..".into(),
            path: p.to_path_buf(),
            is_dir: true,
            is_link: false,
            broken: false,
            size: 0,
            modified: None,
            is_parent: true,
        })
    }

    /// Build an entry for `path` (None if it cannot even be `lstat`ed).
    pub fn from_path(path: &Path) -> Option<Entry> {
        let lmeta = fs::symlink_metadata(path).ok()?;
        let is_link = lmeta.file_type().is_symlink();
        let (meta, broken) = if is_link {
            match fs::metadata(path) {
                Ok(m) => (m, false),
                Err(_) => (lmeta, true),
            }
        } else {
            (lmeta, false)
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        Some(Entry {
            name,
            path: path.to_path_buf(),
            is_dir: meta.is_dir(),
            is_link,
            broken,
            size: if meta.is_dir() { 0 } else { meta.len() },
            modified: meta.modified().ok(),
            is_parent: false,
        })
    }

    pub fn is_hidden(&self) -> bool {
        if self.name.starts_with('.') {
            return true;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if let Ok(m) = fs::symlink_metadata(&self.path) {
                return m.file_attributes() & 0x2 != 0;
            }
        }
        false
    }

    pub fn kind(&self) -> String {
        kind_of(self)
    }

    pub fn size_text(&self) -> String {
        if self.is_dir {
            String::new()
        } else {
            human_size(self.size)
        }
    }

    pub fn time_text(&self) -> String {
        self.modified.map(format_time).unwrap_or_default()
    }
}

pub struct Listing {
    pub entries: Vec<Entry>,
    pub truncated: bool,
    /// Entries that vanished or could not be examined.
    pub unreadable: usize,
}

/// Read a directory (without the ".." row). Errors (permissions, not a directory) are returned,
/// never panicked on.
pub fn read_dir(dir: &Path, show_hidden: bool) -> io::Result<Listing> {
    let rd = fs::read_dir(dir)?;
    let mut out = Listing {
        entries: Vec::new(),
        truncated: false,
        unreadable: 0,
    };
    for item in rd {
        let Ok(item) = item else {
            out.unreadable += 1;
            continue;
        };
        if out.entries.len() >= MAX_ENTRIES {
            out.truncated = true;
            break;
        }
        match Entry::from_path(&item.path()) {
            Some(e) if show_hidden || !e.is_hidden() => out.entries.push(e),
            Some(_) => {}
            None => out.unreadable += 1,
        }
    }
    Ok(out)
}

/// Immediate sub-directories of `dir`, sorted by name (for the tree).
pub fn subdirs(dir: &Path, show_hidden: bool) -> Vec<Entry> {
    let mut v: Vec<Entry> = read_dir(dir, show_hidden)
        .map(|l| l.entries)
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.is_dir)
        .collect();
    sort_entries(&mut v, SortKey::Name, true);
    v
}

// ------------------------------------------------------------------ sorting

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Kind,
}

impl SortKey {
    /// Table column index -> key (0 Name, 1 Size, 2 Modified, 3 Kind).
    pub fn from_column(c: usize) -> SortKey {
        match c {
            1 => SortKey::Size,
            2 => SortKey::Modified,
            3 => SortKey::Kind,
            _ => SortKey::Name,
        }
    }
    pub fn column(self) -> usize {
        match self {
            SortKey::Name => 0,
            SortKey::Size => 1,
            SortKey::Modified => 2,
            SortKey::Kind => 3,
        }
    }
}

fn name_key(e: &Entry) -> String {
    e.name.to_lowercase()
}

/// Folders first, then files; within each group by `key` (ascending or not), ties by name.
/// Folders have no size or kind of their own, so for those keys they stay in name order.
pub fn sort_entries(v: &mut [Entry], key: SortKey, ascending: bool) {
    use std::cmp::Ordering;
    v.sort_by(|a, b| {
        if a.is_dir != b.is_dir {
            return if a.is_dir {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        let by_name = name_key(a)
            .cmp(&name_key(b))
            .then_with(|| a.name.cmp(&b.name));
        let main = match key {
            SortKey::Name => by_name,
            SortKey::Size if !a.is_dir => a.size.cmp(&b.size),
            SortKey::Modified => a.modified.cmp(&b.modified),
            SortKey::Kind if !a.is_dir => kind_of(a).cmp(&kind_of(b)),
            _ => return by_name,
        };
        let main = if ascending { main } else { main.reverse() };
        main.then(by_name)
    });
}

// ------------------------------------------------------------------ formatting

/// 0 -> "0 B", 1536 -> "1.5 KB", 10 MiB -> "10 MB".
pub fn human_size(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let mut v = n as f64;
    let mut unit = 0;
    const U: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    while v >= 1024.0 && unit < U.len() {
        v /= 1024.0;
        unit += 1;
    }
    if v < 9.95 {
        format!("{:.1} {}", v, U[unit - 1])
    } else {
        format!("{:.0} {}", v, U[unit - 1])
    }
}

/// "YYYY-MM-DD HH:MM" in UTC (the std library has no time zone support).
pub fn format_time(t: SystemTime) -> String {
    let secs = match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

/// Days since 1970-01-01 -> (year, month, day), proleptic Gregorian (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// Shorten `path` to at most `max` characters by replacing its start with an ellipsis.
pub fn ellipsize_path(path: &str, max: usize) -> String {
    let n = path.chars().count();
    if n <= max || max < 2 {
        return path.to_string();
    }
    let tail: String = path.chars().skip(n - (max - 1)).collect();
    format!("\u{2026}{tail}")
}

pub fn kind_of(e: &Entry) -> String {
    if e.is_parent {
        return String::new();
    }
    if e.broken {
        return "Broken link".into();
    }
    if e.is_dir {
        return if e.is_link {
            "Folder link".into()
        } else {
            "Folder".into()
        };
    }
    let ext = Path::new(&e.name)
        .extension()
        .map(|x| x.to_string_lossy().to_lowercase());
    let k = match ext.as_deref() {
        Some("rs") => "Rust source",
        Some("c" | "h") => "C source",
        Some("cpp" | "cc" | "hpp") => "C++ source",
        Some("py") => "Python script",
        Some("sh") => "Shell script",
        Some("js" | "ts") => "Script",
        Some("md" | "markdown") => "Markdown",
        Some("txt" | "log") => "Text",
        Some("toml" | "yaml" | "yml" | "json" | "ini" | "cfg") => "Config",
        Some("html" | "htm" | "css" | "xml") => "Markup",
        Some("png" | "jpg" | "jpeg" | "gif" | "bmp" | "ppm" | "pgm" | "svg" | "ico") => "Image",
        Some("zip" | "tar" | "gz" | "xz" | "bz2" | "7z" | "zst") => "Archive",
        Some("pdf") => "PDF",
        Some("lock") => "Lock file",
        Some("o" | "a" | "so" | "dll" | "exe" | "bin") => "Binary",
        Some(x) if x.len() <= 5 => return format!("{} file", x.to_uppercase()),
        _ => "File",
    };
    if e.is_link {
        format!("{k} link")
    } else {
        k.to_string()
    }
}

/// One-line description for the status bar.
pub fn describe(e: &Entry) -> String {
    if e.is_parent {
        return format!("Parent folder {}", e.path.display());
    }
    let mut s = e.name.clone();
    if e.is_dir {
        s.push_str(" (folder)");
    } else {
        s.push_str(&format!(" - {} ({} bytes)", human_size(e.size), e.size));
    }
    if !e.time_text().is_empty() {
        s.push_str(&format!(" - modified {} UTC", e.time_text()));
    }
    if e.is_link {
        match fs::read_link(&e.path) {
            Ok(t) => s.push_str(&format!(" - link to {}", t.display())),
            Err(_) => s.push_str(" - link"),
        }
    }
    s
}

// ------------------------------------------------------------------ operations

/// A name usable for a single path component.
pub fn validate_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("The name is empty".into());
    }
    if name == "." || name == ".." {
        return Err("\".\" and \"..\" are not valid names".into());
    }
    if name.contains(['/', '\\', '\0']) {
        return Err("The name may not contain / \\ or NUL".into());
    }
    Ok(())
}

pub fn make_dir(parent: &Path, name: &str) -> Result<PathBuf, String> {
    validate_name(name)?;
    let p = parent.join(name);
    if fs::symlink_metadata(&p).is_ok() {
        return Err(format!("\"{name}\" already exists"));
    }
    fs::create_dir(&p).map_err(|e| format!("Cannot create \"{name}\": {e}"))?;
    Ok(p)
}

pub fn rename_path(path: &Path, new_name: &str) -> Result<PathBuf, String> {
    validate_name(new_name)?;
    let dest = path.with_file_name(new_name);
    if dest == path {
        return Ok(dest);
    }
    if fs::symlink_metadata(&dest).is_ok() {
        return Err(format!("\"{new_name}\" already exists"));
    }
    fs::rename(path, &dest).map_err(|e| format!("Cannot rename: {e}"))?;
    Ok(dest)
}

/// Where `src` would land when copied/moved into `dst_dir`.
pub fn dest_for(src: &Path, dst_dir: &Path) -> Option<PathBuf> {
    Some(dst_dir.join(src.file_name()?))
}

/// Remove a file, link (never following it) or directory tree.
pub fn remove_path(p: &Path) -> io::Result<()> {
    let m = fs::symlink_metadata(p)?;
    if m.is_dir() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p).or_else(|e| fs::remove_dir(p).map_err(|_| e))
    }
}

fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    let m = fs::symlink_metadata(src)?;
    if m.file_type().is_symlink() {
        #[cfg(unix)]
        {
            return std::os::unix::fs::symlink(fs::read_link(src)?, dst);
        }
        #[cfg(not(unix))]
        {
            if src.is_dir() {
                return copy_tree_dir(src, dst);
            }
            return fs::copy(src, dst).map(|_| ());
        }
    }
    if m.is_dir() {
        return copy_tree_dir(src, dst);
    }
    fs::copy(src, dst).map(|_| ())
}

fn copy_tree_dir(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir(dst)?;
    for item in fs::read_dir(src)? {
        let item = item?;
        copy_tree(&item.path(), &dst.join(item.file_name()))?;
    }
    Ok(())
}

/// Check that `src` can be copied/moved into `dst_dir`: not onto itself, not into its own subtree.
pub fn check_transfer(src: &Path, dst_dir: &Path) -> Result<PathBuf, String> {
    let dest = dest_for(src, dst_dir).ok_or("Nothing to transfer")?;
    if dest == src {
        return Err("Source and destination are the same".into());
    }
    if src.is_dir() && dst_dir.starts_with(src) {
        return Err("Cannot copy or move a folder into itself".into());
    }
    Ok(dest)
}

/// Copy `src` into `dst_dir`. The destination must not exist (the caller confirms and removes it
/// first when overwriting). Returns the new path.
pub fn copy_into(src: &Path, dst_dir: &Path) -> Result<PathBuf, String> {
    let dest = check_transfer(src, dst_dir)?;
    if fs::symlink_metadata(&dest).is_ok() {
        return Err(format!(
            "\"{}\" already exists",
            dest.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    copy_tree(src, &dest).map_err(|e| {
        let _ = remove_path(&dest); // do not leave a half-copied tree behind
        format!("Copy failed: {e}")
    })?;
    Ok(dest)
}

/// Move `src` into `dst_dir`: a rename where possible, else copy then delete.
pub fn move_into(src: &Path, dst_dir: &Path) -> Result<PathBuf, String> {
    let dest = check_transfer(src, dst_dir)?;
    if fs::symlink_metadata(&dest).is_ok() {
        return Err(format!(
            "\"{}\" already exists",
            dest.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    if fs::rename(src, &dest).is_ok() {
        return Ok(dest);
    }
    copy_tree(src, &dest).map_err(|e| {
        let _ = remove_path(&dest);
        format!("Move failed: {e}")
    })?;
    remove_path(src).map_err(|e| format!("Copied, but cannot remove the original: {e}"))?;
    Ok(dest)
}

// ------------------------------------------------------------------ preview

pub enum Preview {
    Text { text: String, truncated: bool },
    Hex { text: String, total: u64 },
    Dir(String),
    Image { img: ImageData, info: String },
    Error(String),
}

impl Preview {
    /// The text shown in the preview TextArea.
    pub fn body(&self) -> &str {
        match self {
            Preview::Text { text, .. } | Preview::Hex { text, .. } => text,
            Preview::Dir(s) | Preview::Error(s) => s,
            Preview::Image { info, .. } => info,
        }
    }
    /// Number of bytes shown (for the trace / status).
    pub fn shown_bytes(&self) -> usize {
        match self {
            Preview::Text { text, .. } | Preview::Hex { text, .. } => text.len(),
            _ => 0,
        }
    }
}

pub fn preview(e: &Entry) -> Preview {
    if e.is_dir {
        return Preview::Dir(dir_summary(&e.path));
    }
    if e.broken {
        return Preview::Error(format!(
            "Broken symbolic link: {}",
            fs::read_link(&e.path)
                .map(|t| t.display().to_string())
                .unwrap_or_default()
        ));
    }
    let f = match fs::File::open(&e.path) {
        Ok(f) => f,
        Err(err) => return Preview::Error(format!("Cannot open {}: {err}", e.name)),
    };
    let mut buf = Vec::new();
    // `take` + read_to_end copes with special files that report a size of 0
    if let Err(err) = f.take(PREVIEW_LIMIT as u64 + 1).read_to_end(&mut buf) {
        return Preview::Error(format!("Cannot read {}: {err}", e.name));
    }
    let truncated = buf.len() > PREVIEW_LIMIT;
    buf.truncate(PREVIEW_LIMIT);
    if let Some(p) = image_preview(e, &buf) {
        return p;
    }
    if looks_binary(&buf) {
        let total = e.size.max(buf.len() as u64);
        return Preview::Hex {
            text: hex_dump(&buf[..buf.len().min(HEX_LIMIT)], total),
            total,
        };
    }
    Preview::Text {
        text: String::from_utf8_lossy(&buf).into_owned(),
        truncated,
    }
}

fn image_preview(e: &Entry, head: &[u8]) -> Option<Preview> {
    let ext = Path::new(&e.name)
        .extension()?
        .to_string_lossy()
        .to_lowercase();
    if !matches!(ext.as_str(), "ppm" | "pnm" | "bmp" | "png") || e.size > IMAGE_MAX_BYTES {
        return None;
    }
    let bytes = if e.size as usize <= head.len() {
        head.to_vec()
    } else {
        fs::read(&e.path).ok()?
    };
    let img = if ext == "bmp" {
        parse_bmp(&bytes)?
    } else if ext == "png" {
        parse_png(&bytes)?
    } else {
        parse_ppm(&bytes)?
    };
    let info = format!(
        "{} image, {} x {} pixels, {}\n",
        ext.to_uppercase(),
        img.w,
        img.h,
        human_size(e.size)
    );
    Some(Preview::Image { img, info })
}

/// Heuristic: NUL bytes, or too many undecodable sequences, mean "not text".
pub fn looks_binary(buf: &[u8]) -> bool {
    let head = &buf[..buf.len().min(8192)];
    if head.contains(&0) {
        return true;
    }
    let s = String::from_utf8_lossy(head);
    let bad = s.chars().filter(|c| *c == '\u{FFFD}').count();
    // a multi-byte character cut by the window must not count; require a real fraction
    bad > 4 && bad * 100 > head.len()
}

pub fn hex_dump(bytes: &[u8], total: u64) -> String {
    let mut s = String::with_capacity(bytes.len() * 4 + 64);
    s.push_str(&format!(
        "Binary file, {} ({} bytes); hex dump of the first {} bytes\n\n",
        human_size(total),
        total,
        bytes.len()
    ));
    for (i, chunk) in bytes.chunks(16).enumerate() {
        s.push_str(&format!("{:08x}  ", i * 16));
        for j in 0..16 {
            match chunk.get(j) {
                Some(b) => s.push_str(&format!("{b:02x} ")),
                None => s.push_str("   "),
            }
            if j == 7 {
                s.push(' ');
            }
        }
        s.push_str(" |");
        s.extend(chunk.iter().map(|&b| {
            if (0x20..0x7f).contains(&b) {
                b as char
            } else {
                '.'
            }
        }));
        s.push_str("|\n");
    }
    s
}

pub fn dir_summary(dir: &Path) -> String {
    match read_dir(dir, true) {
        Err(e) => format!("Cannot read {}: {e}", dir.display()),
        Ok(l) => {
            let dirs = l.entries.iter().filter(|e| e.is_dir).count();
            let files = l.entries.len() - dirs;
            let total: u64 = l.entries.iter().filter(|e| !e.is_dir).map(|e| e.size).sum();
            let mut s = format!(
                "Folder {}\n\n{} items (including hidden): {} folders, {} files\nFiles total {} ({} bytes)\n",
                dir.display(),
                l.entries.len(),
                dirs,
                files,
                human_size(total),
                total
            );
            if l.truncated {
                s.push_str(&format!("(only the first {MAX_ENTRIES} entries counted)\n"));
            }
            if l.unreadable > 0 {
                s.push_str(&format!("{} entries could not be examined\n", l.unreadable));
            }
            s
        }
    }
}

// ------------------------------------------------------------------ tiny image decoders

fn rgb_to_rgba(w: u32, h: u32, rgb: impl Iterator<Item = [u8; 3]>) -> Option<ImageData> {
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for p in rgb {
        rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
    }
    (rgba.len() == w as usize * h as usize * 4).then_some(ImageData { w, h, rgba })
}

/// Binary (P6) and ASCII (P3) PPM with maxval <= 255.
pub fn parse_ppm(b: &[u8]) -> Option<ImageData> {
    let mut pos = 0;
    let token = |pos: &mut usize| -> Option<String> {
        loop {
            while *pos < b.len() && b[*pos].is_ascii_whitespace() {
                *pos += 1;
            }
            if b.get(*pos) == Some(&b'#') {
                while *pos < b.len() && b[*pos] != b'\n' {
                    *pos += 1;
                }
            } else {
                break;
            }
        }
        let start = *pos;
        while *pos < b.len() && !b[*pos].is_ascii_whitespace() {
            *pos += 1;
        }
        (*pos > start).then(|| String::from_utf8_lossy(&b[start..*pos]).into_owned())
    };
    let magic = token(&mut pos)?;
    let w: u32 = token(&mut pos)?.parse().ok()?;
    let h: u32 = token(&mut pos)?.parse().ok()?;
    let max: u32 = token(&mut pos)?.parse().ok()?;
    if w == 0 || h == 0 || w > IMAGE_MAX_EDGE || h > IMAGE_MAX_EDGE || max == 0 || max > 255 {
        return None;
    }
    let scale = |v: u32| (v.min(max) * 255 / max) as u8;
    let n = w as usize * h as usize;
    match magic.as_str() {
        "P6" => {
            pos += 1; // the single whitespace after maxval
            let data = b.get(pos..pos + n * 3)?;
            rgb_to_rgba(
                w,
                h,
                data.chunks_exact(3)
                    .map(|c| [scale(c[0] as u32), scale(c[1] as u32), scale(c[2] as u32)]),
            )
        }
        "P3" => {
            let mut px = Vec::with_capacity(n);
            for _ in 0..n {
                let mut c = [0u8; 3];
                for v in &mut c {
                    *v = scale(token(&mut pos)?.parse().ok()?);
                }
                px.push(c);
            }
            rgb_to_rgba(w, h, px.into_iter())
        }
        _ => None,
    }
}

/// Uncompressed 24/32-bit BMP (BITMAPINFOHEADER or later).
pub fn parse_bmp(b: &[u8]) -> Option<ImageData> {
    let u16le = |o: usize| b.get(o..o + 2).map(|s| u16::from_le_bytes([s[0], s[1]]));
    let u32le = |o: usize| {
        b.get(o..o + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    };
    if b.get(..2)? != b"BM" || u32le(14)? < 40 {
        return None;
    }
    let off = u32le(10)? as usize;
    let w = u32le(18)? as i32;
    let hh = u32le(22)? as i32;
    let bpp = u16le(28)?;
    if u32le(30)? != 0 || !(bpp == 24 || bpp == 32) || w <= 0 || hh == 0 {
        return None;
    }
    let (w, h, top_down) = (w as u32, hh.unsigned_abs(), hh < 0);
    if w > IMAGE_MAX_EDGE || h > IMAGE_MAX_EDGE {
        return None;
    }
    let bytes_pp = bpp as usize / 8;
    let stride = (w as usize * bytes_pp).div_ceil(4) * 4;
    let mut rgba = vec![0u8; w as usize * h as usize * 4];
    for y in 0..h as usize {
        let row = b.get(off + y * stride..off + y * stride + w as usize * bytes_pp)?;
        let dy = if top_down { y } else { h as usize - 1 - y };
        for x in 0..w as usize {
            let s = &row[x * bytes_pp..];
            let d = (dy * w as usize + x) * 4;
            rgba[d..d + 4].copy_from_slice(&[s[2], s[1], s[0], 255]);
        }
    }
    Some(ImageData { w, h, rgba })
}

// ------------------------------------------------------------------ PNG (inflate + unfilter)

/// A minimal DEFLATE decoder (RFC 1951): stored, fixed and dynamic blocks. `limit` bounds the output.
fn inflate(data: &[u8], limit: usize) -> Option<Vec<u8>> {
    struct Bits<'a> {
        d: &'a [u8],
        pos: usize,
        bit: u32,
    }
    impl Bits<'_> {
        fn get(&mut self, n: u32) -> Option<u32> {
            let mut v = 0;
            for i in 0..n {
                let byte = *self.d.get(self.pos)?;
                v |= (((byte >> self.bit) & 1) as u32) << i;
                self.bit += 1;
                if self.bit == 8 {
                    self.bit = 0;
                    self.pos += 1;
                }
            }
            Some(v)
        }
    }
    // canonical Huffman table: (count per length, symbols sorted by code)
    struct Huff {
        count: [u16; 16],
        sym: Vec<u16>,
    }
    fn build(lens: &[u8]) -> Huff {
        let mut count = [0u16; 16];
        for &l in lens {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut offs = [0u16; 16];
        for i in 1..16 {
            offs[i] = offs[i - 1] + count[i - 1];
        }
        let mut sym = vec![0u16; lens.len()];
        for (s, &l) in lens.iter().enumerate() {
            if l != 0 {
                sym[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Huff { count, sym }
    }
    fn decode(b: &mut Bits, h: &Huff) -> Option<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.get(1)? as i32;
            let c = h.count[len] as i32;
            if code - c < first {
                return h.sym.get((index + (code - first)) as usize).copied();
            }
            index += c;
            first = (first + c) << 1;
            code <<= 1;
        }
        None
    }
    const LBASE: [u16; 29] = [
        3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115,
        131, 163, 195, 227, 258,
    ];
    const LEXT: [u8; 29] = [
        0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
    ];
    const DBASE: [u16; 30] = [
        1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
        2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
    ];
    const DEXT: [u8; 30] = [
        0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12,
        13, 13,
    ];
    let mut b = Bits {
        d: data,
        pos: 0,
        bit: 0,
    };
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = b.get(1)?;
        match b.get(2)? {
            0 => {
                if b.bit != 0 {
                    b.bit = 0;
                    b.pos += 1;
                }
                let n = u16::from_le_bytes([*b.d.get(b.pos)?, *b.d.get(b.pos + 1)?]) as usize;
                let blk = b.d.get(b.pos + 4..b.pos + 4 + n)?;
                if out.len() + n > limit {
                    return None;
                }
                out.extend_from_slice(blk);
                b.pos += 4 + n;
            }
            t @ (1 | 2) => {
                let (lit, dist) = if t == 1 {
                    let mut l = [8u8; 288];
                    l[144..256].fill(9);
                    l[256..280].fill(7);
                    (build(&l), build(&[5u8; 30]))
                } else {
                    let nlen = b.get(5)? as usize + 257;
                    let ndist = b.get(5)? as usize + 1;
                    let ncode = b.get(4)? as usize + 4;
                    const ORDER: [usize; 19] = [
                        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                    ];
                    let mut cl = [0u8; 19];
                    for &o in &ORDER[..ncode] {
                        cl[o] = b.get(3)? as u8;
                    }
                    let clh = build(&cl);
                    let mut lens = Vec::with_capacity(nlen + ndist);
                    while lens.len() < nlen + ndist {
                        let s = decode(&mut b, &clh)?;
                        let (v, rep) = match s {
                            0..=15 => (s as u8, 1),
                            16 => (*lens.last()?, 3 + b.get(2)? as usize),
                            17 => (0, 3 + b.get(3)? as usize),
                            _ => (0, 11 + b.get(7)? as usize),
                        };
                        lens.resize(lens.len() + rep, v);
                    }
                    if lens.len() > nlen + ndist {
                        return None;
                    }
                    (build(&lens[..nlen]), build(&lens[nlen..]))
                };
                loop {
                    let s = decode(&mut b, &lit)? as usize;
                    if s < 256 {
                        out.push(s as u8);
                    } else if s == 256 {
                        break;
                    } else {
                        let s = s - 257;
                        let len = *LBASE.get(s)? as usize + b.get(*LEXT.get(s)? as u32)? as usize;
                        let d = decode(&mut b, &dist)? as usize;
                        let dd = *DBASE.get(d)? as usize + b.get(*DEXT.get(d)? as u32)? as usize;
                        if dd > out.len() {
                            return None;
                        }
                        for _ in 0..len {
                            out.push(out[out.len() - dd]);
                        }
                    }
                    if out.len() > limit {
                        return None;
                    }
                }
            }
            _ => return None,
        }
        if last == 1 {
            return Some(out);
        }
    }
}

/// Non-interlaced PNG of any colour type, bit depth 1 to 16 (16-bit keeps the high byte).
pub fn parse_png(b: &[u8]) -> Option<ImageData> {
    if b.get(..8)? != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let (mut pos, mut hdr, mut plte, mut trns) = (8, None, Vec::new(), Vec::new());
    let mut idat = Vec::new();
    while let Some(h) = b.get(pos..pos + 8) {
        let len = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as usize;
        let data = b.get(pos + 8..pos + 8 + len)?;
        match &h[4..8] {
            b"IHDR" => hdr = Some(data.to_vec()),
            b"PLTE" => plte = data.to_vec(),
            b"tRNS" => trns = data.to_vec(),
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    let hdr = hdr.filter(|h| h.len() == 13)?;
    let w = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
    let h = u32::from_be_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
    let (depth, ctype, interlace) = (hdr[8] as usize, hdr[9], hdr[12]);
    let chans = match ctype {
        0 | 3 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return None,
    };
    if w == 0 || h == 0 || w > IMAGE_MAX_EDGE || h > IMAGE_MAX_EDGE || interlace != 0 {
        return None;
    }
    if !matches!(depth, 1 | 2 | 4 | 8 | 16) || (ctype == 3 && plte.is_empty()) {
        return None;
    }
    let (w, h) = (w as usize, h as usize);
    let bpp = (chans * depth).div_ceil(8); // bytes per pixel for filtering
    let stride = (w * chans * depth).div_ceil(8);
    // zlib: 2 header bytes, deflate stream, adler32 (ignored)
    let raw = inflate(idat.get(2..)?, (stride + 1) * h)?;
    if raw.len() < (stride + 1) * h {
        return None;
    }
    let mut cur = vec![0u8; stride];
    let mut prev = vec![0u8; stride];
    let mut rgba = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let line = &raw[y * (stride + 1)..(y + 1) * (stride + 1)];
        cur.copy_from_slice(&line[1..]);
        for i in 0..stride {
            let a = if i >= bpp { cur[i - bpp] as i32 } else { 0 };
            let up = prev[i] as i32;
            let c = if i >= bpp { prev[i - bpp] as i32 } else { 0 };
            let add = match line[0] {
                0 => 0,
                1 => a,
                2 => up,
                3 => (a + up) / 2,
                4 => {
                    let p = a + up - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - up).abs(), (p - c).abs());
                    if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        up
                    } else {
                        c
                    }
                }
                _ => return None,
            };
            cur[i] = cur[i].wrapping_add(add as u8);
        }
        // sample n of the row, scaled to 8 bits (palette indexes stay raw)
        let sample = |n: usize| -> u8 {
            match depth {
                8 => cur[n],
                16 => cur[n * 2],
                d => {
                    let per = 8 / d;
                    let v = (cur[n / per] >> (8 - d * (n % per + 1))) & ((1 << d) - 1);
                    if ctype == 3 {
                        v
                    } else {
                        (v as u32 * 255 / ((1 << d) - 1)) as u8
                    }
                }
            }
        };
        for x in 0..w {
            let px = match ctype {
                0 => {
                    let g = sample(x);
                    [g, g, g, 255]
                }
                2 => [sample(x * 3), sample(x * 3 + 1), sample(x * 3 + 2), 255],
                3 => {
                    let i = sample(x) as usize;
                    let p = plte.get(i * 3..i * 3 + 3)?;
                    [p[0], p[1], p[2], trns.get(i).copied().unwrap_or(255)]
                }
                4 => {
                    let g = sample(x * 2);
                    [g, g, g, sample(x * 2 + 1)]
                }
                _ => [
                    sample(x * 4),
                    sample(x * 4 + 1),
                    sample(x * 4 + 2),
                    sample(x * 4 + 3),
                ],
            };
            rgba.extend_from_slice(&px);
        }
        std::mem::swap(&mut cur, &mut prev);
    }
    Some(ImageData {
        w: w as u32,
        h: h as u32,
        rgba,
    })
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    struct Tmp(PathBuf);
    impl Tmp {
        fn new() -> Tmp {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "rungui-fm-test-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
        fn file(&self, rel: &str, data: &[u8]) -> PathBuf {
            let p = self.0.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, data).unwrap();
            p
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(l: &[Entry]) -> Vec<&str> {
        l.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn sizes_and_times() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(1023), "1023 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(10 * 1024), "10 KB");
        assert_eq!(human_size(5 * 1024 * 1024 + 300_000), "5.3 MB");
        assert_eq!(human_size(3 * 1024 * 1024 * 1024), "3.0 GB");
        assert_eq!(human_size(u64::MAX), "16384 PB");
        assert_eq!(format_time(UNIX_EPOCH), "1970-01-01 00:00");
        assert_eq!(
            format_time(UNIX_EPOCH + Duration::from_secs(1_000_000_000)),
            "2001-09-09 01:46"
        );
        assert_eq!(
            format_time(UNIX_EPOCH + Duration::from_secs(1_709_164_800)),
            "2024-02-29 00:00"
        ); // leap day
        assert_eq!(
            format_time(UNIX_EPOCH - Duration::from_secs(86_400)),
            "1969-12-31 00:00"
        );
    }

    #[test]
    fn ellipsis() {
        assert_eq!(ellipsize_path("/a/b", 10), "/a/b");
        assert_eq!(
            ellipsize_path("/home/user/projects/x", 10),
            "\u{2026}rojects/x"
        );
        assert_eq!(ellipsize_path("日本語のパス名", 4), "\u{2026}パス名");
    }

    #[test]
    fn names_are_validated() {
        for bad in ["", "  ", ".", "..", "a/b", "a\\b", "a\0b"] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        for ok in ["a", "日本語.txt", ".hidden", "with space", "a.b.c"] {
            assert!(validate_name(ok).is_ok(), "{ok:?}");
        }
    }

    #[test]
    fn listing_hides_dotfiles_and_sorts() {
        let t = Tmp::new();
        t.file("b.txt", b"bb");
        t.file("A.txt", b"a");
        t.file(".hidden", b"h");
        t.file("zdir/inner", b"x");
        t.file("big.bin", &vec![1u8; 5000]);
        fs::create_dir(t.0.join("Adir")).unwrap();
        let mut l = read_dir(&t.0, false).unwrap().entries;
        assert_eq!(l.len(), 5);
        assert!(!l.iter().any(|e| e.name == ".hidden"));
        assert_eq!(read_dir(&t.0, true).unwrap().entries.len(), 6);
        sort_entries(&mut l, SortKey::Name, true);
        assert_eq!(names(&l), ["Adir", "zdir", "A.txt", "b.txt", "big.bin"]);
        sort_entries(&mut l, SortKey::Name, false); // folders stay first
        assert_eq!(names(&l), ["zdir", "Adir", "big.bin", "b.txt", "A.txt"]);
        sort_entries(&mut l, SortKey::Size, false);
        assert_eq!(names(&l), ["Adir", "zdir", "big.bin", "b.txt", "A.txt"]);
        sort_entries(&mut l, SortKey::Size, true);
        assert_eq!(names(&l), ["Adir", "zdir", "A.txt", "b.txt", "big.bin"]);
        sort_entries(&mut l, SortKey::Kind, true);
        assert_eq!(names(&l)[..2], ["Adir", "zdir"]);
        assert_eq!(names(&l)[2], "big.bin"); // "Binary" < "Text"
        assert!(read_dir(&t.0.join("missing"), false).is_err());
        assert!(read_dir(&t.0.join("A.txt"), false).is_err());
    }

    #[test]
    fn columns_round_trip() {
        for c in 0..4 {
            assert_eq!(SortKey::from_column(c).column(), c);
        }
    }

    #[test]
    fn copy_move_delete_rename() {
        let t = Tmp::new();
        t.file("src/a.txt", b"hello");
        t.file("src/sub/b.txt", b"world");
        fs::create_dir(t.0.join("dst")).unwrap();
        let (src, dst) = (t.0.join("src"), t.0.join("dst"));

        let copied = copy_into(&src, &dst).unwrap();
        assert_eq!(copied, dst.join("src"));
        assert_eq!(fs::read(dst.join("src/sub/b.txt")).unwrap(), b"world");
        assert!(
            copy_into(&src, &dst)
                .unwrap_err()
                .contains("already exists")
        );
        assert!(copy_into(&src, &src).unwrap_err().contains("itself"));
        assert!(
            copy_into(&src, &src.join("sub"))
                .unwrap_err()
                .contains("itself")
        );
        assert!(
            copy_into(&src.join("a.txt"), &src)
                .unwrap_err()
                .contains("same")
        );

        let moved = move_into(&dst.join("src"), &t.0).unwrap_err(); // t.0/src exists already
        assert!(moved.contains("already exists"));
        let one = move_into(&src.join("a.txt"), &dst).unwrap();
        assert_eq!(fs::read(one).unwrap(), b"hello");
        assert!(!src.join("a.txt").exists());

        let r = rename_path(&dst.join("a.txt"), "renamed.txt").unwrap();
        assert!(r.exists());
        assert!(
            rename_path(&r, "src")
                .unwrap_err()
                .contains("already exists")
        );
        assert!(rename_path(&dst.join("src"), "src").is_ok()); // renaming to itself is a no-op
        assert!(rename_path(&dst.join("src"), "x/y").is_err());

        let d = make_dir(&dst, "new").unwrap();
        assert!(d.is_dir());
        assert!(make_dir(&dst, "new").is_err());
        assert!(make_dir(&dst, "").is_err());
        assert!(make_dir(&dst.join("nope/deeper"), "x").is_err());

        remove_path(&dst).unwrap();
        assert!(!dst.exists());
        assert!(remove_path(&dst).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        use std::os::unix::fs::symlink;
        let t = Tmp::new();
        t.file("real/f.txt", b"data");
        symlink("real", t.0.join("link")).unwrap();
        symlink("missing", t.0.join("dangling")).unwrap();
        let l = read_dir(&t.0, false).unwrap().entries;
        let link = l.iter().find(|e| e.name == "link").unwrap();
        assert!(link.is_link && link.is_dir && !link.broken);
        assert_eq!(link.kind(), "Folder link");
        let dang = l.iter().find(|e| e.name == "dangling").unwrap();
        assert!(dang.is_link && dang.broken && !dang.is_dir);
        assert_eq!(dang.kind(), "Broken link");
        assert!(describe(dang).contains("link to missing"));
        assert!(matches!(preview(dang), Preview::Error(_)));

        fs::create_dir(t.0.join("out")).unwrap();
        let c = copy_into(&t.0.join("link"), &t.0.join("out")).unwrap();
        assert!(fs::symlink_metadata(&c).unwrap().file_type().is_symlink()); // copied as a link
        remove_path(&t.0.join("link")).unwrap(); // deletes the link only
        assert!(t.0.join("real/f.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_names_are_lossy() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let t = Tmp::new();
        let p = t.0.join(OsStr::from_bytes(b"bad-\xff-name.txt"));
        fs::write(&p, b"x").unwrap();
        let l = read_dir(&t.0, false).unwrap().entries;
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].name, "bad-\u{fffd}-name.txt");
        assert_eq!(l[0].path, p); // operations still use the real name
        fs::create_dir(t.0.join("out")).unwrap();
        let c = copy_into(&p, &t.0.join("out")).unwrap();
        assert_eq!(fs::read(c).unwrap(), b"x");
    }

    #[test]
    fn kinds() {
        let mk = |n: &str| Entry {
            name: n.into(),
            path: n.into(),
            is_dir: false,
            is_link: false,
            broken: false,
            size: 1,
            modified: None,
            is_parent: false,
        };
        assert_eq!(kind_of(&mk("a.rs")), "Rust source");
        assert_eq!(kind_of(&mk("a.PNG")), "Image");
        assert_eq!(kind_of(&mk("a.xyz")), "XYZ file");
        assert_eq!(kind_of(&mk("Makefile")), "File");
        assert_eq!(kind_of(&mk(".bashrc")), "File");
    }

    #[test]
    fn previews() {
        let t = Tmp::new();
        let text = t.file("t.txt", "héllo wörld\nsecond line\n".as_bytes());
        let e = Entry::from_path(&text).unwrap();
        match preview(&e) {
            Preview::Text { text, truncated } => {
                assert!(text.starts_with("héllo"));
                assert!(!truncated);
            }
            _ => panic!("expected text"),
        }
        let big = t.file("big.txt", &vec![b'x'; PREVIEW_LIMIT + 100]);
        match preview(&Entry::from_path(&big).unwrap()) {
            Preview::Text { text, truncated } => assert!(truncated && text.len() == PREVIEW_LIMIT),
            _ => panic!(),
        }
        let bin = t.file(
            "b.bin",
            &[
                0, 1, 2, 0xff, b'A', b'B', 0, 0, 9, 10, 11, 12, 13, 14, 15, 16, 17,
            ],
        );
        match preview(&Entry::from_path(&bin).unwrap()) {
            Preview::Hex { text, total } => {
                assert_eq!(total, 17);
                assert!(text.contains("00000000  00 01 02 ff 41 42 00 00  09 0a 0b 0c 0d 0e 0f 10  |....AB..........|"), "{text}");
                assert!(text.contains("00000010  11 "));
            }
            _ => panic!("expected hex"),
        }
        let invalid = t.file("latin1.txt", &[0xe9; 400]); // no NULs, but not UTF-8 at all
        assert!(matches!(
            preview(&Entry::from_path(&invalid).unwrap()),
            Preview::Hex { .. }
        ));
        let empty = t.file("empty", b"");
        assert!(
            matches!(preview(&Entry::from_path(&empty).unwrap()), Preview::Text { ref text, .. } if text.is_empty())
        );
        let d = Entry::from_path(&t.0).unwrap();
        match preview(&d) {
            Preview::Dir(s) => assert!(
                s.contains("5 items (including hidden): 0 folders, 5 files"),
                "{s}"
            ),
            _ => panic!(),
        }
    }

    #[test]
    fn png() {
        // 24x24 RGB, zlib-compressed, all five filter types in rotation
        let png: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x18, 0x00, 0x00, 0x00, 0x18, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x6f, 0x15, 0xaa, 0xaf, 0x00, 0x00, 0x01, 0x4f, 0x49, 0x44, 0x41, 0x54, 0x78,
            0xda, 0xcd, 0xd0, 0xa1, 0x4f, 0x82, 0x01, 0x10, 0x40, 0xf1, 0xfb, 0x50, 0x70, 0xa0,
            0x9b, 0xce, 0x61, 0xb0, 0x50, 0x2c, 0x18, 0xae, 0x10, 0xa4, 0xb0, 0xa9, 0x85, 0x62,
            0xa1, 0x68, 0xb8, 0x42, 0x90, 0xe2, 0xe6, 0x28, 0x16, 0x0a, 0xdb, 0x1b, 0xe5, 0x0a,
            0x41, 0x0b, 0x85, 0x42, 0xb1, 0x50, 0x2c, 0x57, 0x0c, 0x52, 0x28, 0x14, 0x0b, 0xc5,
            0x42, 0xa1, 0x50, 0x2c, 0x14, 0x8b, 0x49, 0xfd, 0x1b, 0x24, 0x7c, 0xdb, 0xcb, 0x2f,
            0xfc, 0x44, 0x84, 0x82, 0x50, 0x14, 0x4a, 0x42, 0x59, 0xa8, 0x08, 0x35, 0xa1, 0x2e,
            0x34, 0x04, 0x13, 0x5a, 0x42, 0x5b, 0xe8, 0x08, 0x3d, 0xa1, 0x2f, 0x0c, 0x84, 0x91,
            0x30, 0x16, 0x42, 0x98, 0x08, 0x33, 0x61, 0x2e, 0x2c, 0x84, 0x95, 0x90, 0x24, 0xf9,
            0xdf, 0x91, 0xfc, 0xbf, 0x4c, 0x92, 0x97, 0x8d, 0xb4, 0x95, 0x39, 0xbc, 0xc8, 0x65,
            0xe5, 0xff, 0x6d, 0xff, 0xfd, 0x64, 0x03, 0x49, 0x56, 0xd9, 0x57, 0x8e, 0x95, 0x13,
            0x45, 0x95, 0xaa, 0x72, 0xa9, 0x5c, 0x29, 0xd7, 0x4a, 0x53, 0xb9, 0x53, 0x1e, 0x94,
            0xae, 0xe2, 0xca, 0x93, 0x32, 0x54, 0x9e, 0x95, 0x17, 0xe5, 0x55, 0x99, 0x2a, 0xef,
            0xca, 0x87, 0xb2, 0x54, 0x3e, 0x95, 0x24, 0x77, 0x96, 0x36, 0xec, 0xec, 0x69, 0xda,
            0xb0, 0x0b, 0x46, 0xd1, 0x28, 0x19, 0x65, 0xa3, 0x62, 0xd4, 0x8c, 0xba, 0xd1, 0x30,
            0xcc, 0x68, 0x19, 0x6d, 0xa3, 0x63, 0xf4, 0x8c, 0xbe, 0x31, 0x30, 0x46, 0xc6, 0xd8,
            0x08, 0x63, 0x62, 0xcc, 0x8c, 0xb9, 0xb1, 0x30, 0x56, 0xc6, 0xda, 0x48, 0x76, 0x6f,
            0xd3, 0x86, 0xbd, 0x73, 0x9e, 0x36, 0xec, 0x7d, 0xe7, 0xd8, 0x39, 0x71, 0xd4, 0xa9,
            0x3a, 0x97, 0xce, 0x95, 0x73, 0xed, 0x34, 0x9d, 0x3b, 0xe7, 0xc1, 0xe9, 0x3a, 0xee,
            0x3c, 0x39, 0x43, 0xe7, 0xd9, 0x79, 0x71, 0x5e, 0x9d, 0xa9, 0xf3, 0xee, 0x7c, 0x38,
            0x4b, 0xe7, 0xd3, 0xf9, 0x72, 0x92, 0x83, 0xc7, 0xb4, 0x61, 0x17, 0x6e, 0xd2, 0x86,
            0x5d, 0x0c, 0x4a, 0x41, 0x39, 0xa8, 0x04, 0xb5, 0xa0, 0x1e, 0x34, 0x02, 0x0b, 0x5a,
            0x41, 0x3b, 0xe8, 0x04, 0xbd, 0xa0, 0x1f, 0x0c, 0x82, 0x51, 0x30, 0x0e, 0x22, 0x98,
            0x04, 0xb3, 0x60, 0x1e, 0x2c, 0x82, 0x55, 0xb0, 0x0e, 0xbe, 0x83, 0xe4, 0xe8, 0x2d,
            0x6d, 0xd8, 0x7b, 0xf7, 0x9b, 0xc1, 0xfe, 0x01, 0x69, 0x83, 0xb6, 0xe0, 0xd8, 0x4c,
            0x2a, 0x85, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let img = parse_png(png).unwrap();
        assert_eq!((img.w, img.h), (24, 24));
        for y in 0..24usize {
            for x in 0..24usize {
                let o = (y * 24 + x) * 4;
                let want = [((x * 10 + y) % 256) as u8, ((y * 9) % 256) as u8, 128, 255];
                assert_eq!(&img.rgba[o..o + 4], &want, "pixel {x},{y}");
            }
        }
        assert!(parse_png(&png[..200]).is_none());
        assert!(parse_png(b"not a png").is_none());
    }

    #[test]
    fn images() {
        let ppm = b"P6\n# comment\n2 2\n255\n\xff\x00\x00\x00\xff\x00\x00\x00\xff\xff\xff\xff";
        let img = parse_ppm(ppm).unwrap();
        assert_eq!((img.w, img.h), (2, 2));
        assert_eq!(&img.rgba[..8], &[255, 0, 0, 255, 0, 255, 0, 255]);
        let p3 = parse_ppm(b"P3 1 1 15 15 0 7").unwrap();
        assert_eq!(&p3.rgba[..4], &[255, 0, 119, 255]);
        assert!(parse_ppm(b"P6\n2 2\n255\nshort").is_none());
        assert!(parse_ppm(b"P6\n99999 2\n255\n").is_none());
        assert!(parse_ppm(b"junk").is_none());

        // 2x1 bottom-up 24-bit BMP: rows padded to 4 bytes
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0; 8]);
        bmp.extend_from_slice(&54u32.to_le_bytes());
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&2i32.to_le_bytes());
        bmp.extend_from_slice(&1i32.to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&24u16.to_le_bytes());
        bmp.extend_from_slice(&[0; 24]);
        bmp.extend_from_slice(&[0, 0, 255, 255, 0, 0, 0, 0]); // BGR red, BGR blue, pad
        let img = parse_bmp(&bmp).unwrap();
        assert_eq!((img.w, img.h), (2, 1));
        assert_eq!(&img.rgba, &[255, 0, 0, 255, 0, 0, 255, 255]);
        assert!(parse_bmp(&bmp[..58]).is_none());
        assert!(parse_bmp(b"not a bmp at all, definitely not").is_none());

        let t = Tmp::new();
        let p = t.file("pic.ppm", ppm);
        match preview(&Entry::from_path(&p).unwrap()) {
            Preview::Image { img, info } => {
                assert!(img.w == 2 && info.contains("PPM image, 2 x 2"))
            }
            _ => panic!("expected image"),
        }
        let p = t.file("fake.ppm", b"P6 not really an image\n");
        assert!(matches!(
            preview(&Entry::from_path(&p).unwrap()),
            Preview::Text { .. }
        ));
    }
}
