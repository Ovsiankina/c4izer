//! The files of each `export -f` text, JSON, theme and static format
//! (P3-400, P3-404), and the R1 output guards: shared, as one source
//! file, by the CLI (`export`) and the editor's export (P3-431), so both
//! write the same bytes under the same names.
//!
//! Depends on: `c4rizer-core`.

// DECISION: the editor cannot depend on the CLI binary, and core's pub
// surface is not the place for a CLI format list, so this file is
// compiled into both crates (`#[path]` in `crates/editor/src/main.rs`).
// Everything here is crate-private in both.

use std::ffi::OsString;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use c4rizer_core::export::{self, Flavour, text};
use c4rizer_core::model::Model;
use c4rizer_core::pipeline::AllViews;

/// The workspace the files are written from: its model and built views,
/// the path it was read from (`None` for stdin) and its text when that is
/// DSL (`structurizr.dsl` of the JSON, P3-415).
pub(crate) struct Source<'a> {
    /// The model.
    pub(crate) model: &'a Model,
    /// Every view built (`pipeline::build_all`).
    pub(crate) all: &'a AllViews,
    /// The file read; names the JSON (`<file stem>.json`, P3-413).
    pub(crate) path: Option<&'a Path>,
    /// The file's text when it is DSL, `None` for a workspace JSON.
    pub(crate) dsl: Option<&'a str>,
}

/// The files of `format` (`plantuml/structurizr`, `plantuml/c4plantuml`,
/// `mermaid`, `websequencediagrams`, `dot`, `ilograph`, `json`, `theme`,
/// `static`), paths relative to the
/// output folder; `None` for any other name. `positions` adds DOT `pos`
/// (P3-411).
pub(crate) fn files(format: &str, source: &Source<'_>, positions: bool) -> Option<Vec<(PathBuf, Vec<u8>)>> {
    let (model, all) = (source.model, source.all);
    let texts = |v: Vec<(String, String)>| -> Vec<(PathBuf, Vec<u8>)> {
        v.into_iter().map(|(n, t)| (PathBuf::from(n), t.into_bytes())).collect()
    };
    Some(match format {
        "plantuml/structurizr" => texts(text::plantuml(model, all, Flavour::Structurizr)),
        "plantuml/c4plantuml" => texts(text::plantuml(model, all, Flavour::C4PlantUml)),
        "mermaid" => texts(text::mermaid(model, all)),
        "websequencediagrams" => texts(text::websequencediagrams(model, all)),
        "dot" => texts(text::dot(model, all, positions)),
        "ilograph" => texts(text::ilograph(model, all)),
        "json" => {
            let stem = source
                .path
                .and_then(Path::file_stem)
                .map_or_else(|| "workspace".to_owned(), |s| s.to_string_lossy().into_owned());
            vec![(PathBuf::from(format!("{stem}.json")), export::json(model, all, source.dsl).into_bytes())]
        }
        "theme" => vec![(PathBuf::from("workspace-theme.json"), export::theme(model).into_bytes())],
        "static" => export::static_site(model, all),
        _ => return None,
    })
}

/// True when `a` and `b` are the same existing file: same device and
/// inode (approver ruling R1), so a hard link or a symlink to `a` is `a`.
// DECISION: off Unix, where no inode is at hand, the canonical paths are
// compared instead (hard links are then not caught).
pub(crate) fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(a), std::fs::metadata(b)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }
    #[cfg(not(unix))]
    {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}

/// The first output that is one of the inputs (R1), with that input.
pub(crate) fn input_hit<'a>(outputs: &'a [PathBuf], inputs: &'a [PathBuf]) -> Option<(&'a Path, &'a Path)> {
    outputs
        .iter()
        .find_map(|out| inputs.iter().find(|i| same_file(i, out)).map(|i| (out.as_path(), i.as_path())))
}

/// The first pair of view keys equal up to ASCII case, in order: their
/// files would overwrite each other on a case-insensitive file system
/// (CLI-1's guard, shared by `export` and the editor's export).
pub(crate) fn case_collision<'k>(keys: &[&'k str]) -> Option<(&'k str, &'k str)> {
    keys.iter()
        .enumerate()
        .find_map(|(i, first)| keys[i + 1..].iter().find(|second| first.eq_ignore_ascii_case(second)).map(|second| (*first, *second)))
}

/// The longest file name the temporary file may have (bytes): the usual
/// limit of one path component.
const NAME_MAX: usize = 255;

/// `bytes` to a temporary file beside `path`, then renamed over it (R1:
/// never written in place); an existing file's permissions are kept. A
/// symlink at `path` is replaced by the file, never written through. The
/// temporary file is created new: one already there (planted, or a
/// symlink) is an error, never written through. On error the temporary
/// file is removed and `path` is untouched.
// DECISION: the temporary name is `.<name>.c4rizer-<pid>.tmp`, `<name>`
// cut at a character boundary so the whole stays within 255 bytes.
pub(crate) fn replace_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "the path names no file"))?;
    let suffix = format!(".c4rizer-{}.tmp", std::process::id());
    let name = name.to_string_lossy();
    let room = NAME_MAX.saturating_sub(1 + suffix.len());
    let mut cut = name.len().min(room);
    while !name.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut tmp_name = OsString::from(".");
    tmp_name.push(&name[..cut]);
    tmp_name.push(&suffix);
    let tmp = dir.join(tmp_name);
    let mut created = false;
    let result = (|| {
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(|e| io::Error::new(e.kind(), format!("cannot create the temporary file {}: {e}", tmp.display())))?;
        created = true;
        out.write_all(bytes)?;
        out.sync_all()?;
        if let Ok(meta) = std::fs::metadata(path) {
            std::fs::set_permissions(&tmp, meta.permissions())?;
        }
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() && created {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("c4rizer-export-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A file planted at the temporary name is never written through.
    #[cfg(unix)]
    #[test]
    fn replace_file_never_follows_a_planted_temporary() {
        let dir = scratch("planted");
        let victim = dir.join("victim.txt");
        std::fs::write(&victim, "keep").unwrap();
        let out = dir.join("out.txt");
        let tmp = dir.join(format!(".out.txt.c4rizer-{}.tmp", std::process::id()));
        std::os::unix::fs::symlink(&victim, &tmp).unwrap();
        let err = replace_file(&out, b"new").unwrap_err();
        assert!(err.to_string().contains(&format!(".out.txt.c4rizer-{}.tmp", std::process::id())), "{err}");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert!(!out.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file name of 255 bytes still gets a temporary name that fits.
    #[test]
    fn replace_file_handles_the_longest_names() {
        let dir = scratch("long");
        let name = format!("{}.puml", "é".repeat(100) + &"v".repeat(50));
        assert_eq!(name.len(), 255);
        let out = dir.join(&name);
        replace_file(&out, b"text").unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "text");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "no temporary file left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn case_collision_finds_keys_equal_up_to_case() {
        assert_eq!(case_collision(&["a", "Ab", "c", "ab"]), Some(("Ab", "ab")));
        assert_eq!(case_collision(&["a", "b"]), None);
    }
}
