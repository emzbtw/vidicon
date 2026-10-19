//! The user's trash (recording ticket 05), per the freedesktop.org trash
//! spec: the home trash only, `$XDG_DATA_HOME/Trash`. A file is moved to its
//! `files` with a `.trashinfo` in `info` naming where it was, so the
//! desktop's trash can restore it. A file on another filesystem fails, as
//! the per-volume `$topdir/.Trash` is not supported.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

/// Moves `file`, an absolute path, into the trash at `trash`, making it if
/// need be. Returns its name there: its own, or `name (2).ts` and on if
/// taken. On an error nothing has moved.
pub fn trash(file: &Path, trash: &Path, now: i64) -> io::Result<String> {
    let (files, info) = (trash.join("files"), trash.join("info"));
    for dir in [&files, &info] {
        DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    let name = file
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?
        .to_string_lossy();
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, format!(".{ext}")),
        _ => (&*name, String::new()),
    };
    let tm = crate::ui::local_tm(now);
    let contents = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}-{:02}-{:02}T{:02}:{:02}:{:02}\n",
        escape(file),
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    );
    for n in 1.. {
        let name = match n {
            1 => name.to_string(),
            n => format!("{stem} ({n}){ext}"),
        };
        // The info file, made first and exclusively, claims the name.
        let info_file = info.join(format!("{name}.trashinfo"));
        let mut f = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&info_file)
        {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        };
        let to = files.join(&name);
        let moved = if to.exists() {
            // Left by something else without its info: try the next name.
            None
        } else {
            Some(
                f.write_all(contents.as_bytes())
                    .and_then(|()| fs::rename(file, &to)),
            )
        };
        match moved {
            Some(Ok(())) => return Ok(name),
            moved => {
                let _ = fs::remove_file(&info_file);
                if let Some(Err(e)) = moved {
                    return Err(e);
                }
            }
        }
    }
    unreachable!()
}

/// `path` as the spec's `Path=` holds it: URI-escaped, `/` kept.
fn escape(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::new();
    for &b in path.as_os_str().as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char);
            }
            b => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The home trash: `$XDG_DATA_HOME/Trash`.
pub fn home_trash() -> PathBuf {
    crate::config::data_home().join("Trash")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_moves_with_its_info_and_a_taken_name_gets_another() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("Trash");
        let file = dir.path().join("Northwind Sport 1 - 20.00.ts");
        fs::write(&file, "one").unwrap();
        let name = trash(&file, &bin, 1_800_000_000).unwrap();
        assert_eq!(name, "Northwind Sport 1 - 20.00.ts");
        assert!(!file.exists());
        assert_eq!(
            fs::read_to_string(bin.join("files").join(&name)).unwrap(),
            "one"
        );
        let info = fs::read_to_string(bin.join(format!("info/{name}.trashinfo"))).unwrap();
        let mut lines = info.lines();
        assert_eq!(lines.next(), Some("[Trash Info]"));
        let path = format!(
            "Path={}/Northwind%20Sport%201%20-%2020.00.ts",
            escape(dir.path())
        );
        assert_eq!(lines.next(), Some(path.as_str()));
        let date = lines.next().unwrap().strip_prefix("DeletionDate=").unwrap();
        assert_eq!((date.len(), &date[4..5], &date[10..11]), (19, "-", "T"));

        // The same name again, and once more with only a stray file there.
        fs::write(&file, "two").unwrap();
        assert_eq!(
            trash(&file, &bin, 0).unwrap(),
            "Northwind Sport 1 - 20.00 (2).ts"
        );
        fs::write(bin.join("files/Northwind Sport 1 - 20.00 (3).ts"), "").unwrap();
        fs::write(&file, "three").unwrap();
        assert_eq!(
            trash(&file, &bin, 0).unwrap(),
            "Northwind Sport 1 - 20.00 (4).ts"
        );
        assert!(
            !bin.join("info/Northwind Sport 1 - 20.00 (3).ts.trashinfo")
                .exists()
        );
    }

    #[test]
    fn a_failed_move_leaves_the_file_and_no_info() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("Trash");
        let gone = dir.path().join("gone.ts");
        assert!(trash(&gone, &bin, 0).is_err());
        assert_eq!(fs::read_dir(bin.join("info")).unwrap().count(), 0);
    }

    #[test]
    fn paths_are_uri_escaped() {
        assert_eq!(
            escape(Path::new("/home/a/Vidicon/Ma Chaîne - 100%.ts")),
            "/home/a/Vidicon/Ma%20Cha%C3%AEne%20-%20100%25.ts"
        );
    }
}
