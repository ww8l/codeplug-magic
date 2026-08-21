//! One check in front of every command that writes a file the operator named.
//!
//! ~20 commands take a `path: String` straight across the IPC boundary and
//! hand it to `std::fs::write`. In practice each one comes from a save dialog,
//! which is why #91 called this defence in depth rather than a live hole and
//! deliberately left it out of a hygiene batch. It is still the shape of thing
//! worth closing before a repo goes public: nothing in the app checked that the
//! file it was about to overwrite was even the KIND of file that command
//! produces.
//!
//! The rule is deliberately narrow. It does not restrict WHERE a file may be
//! written — an operator exports a codeplug to wherever they like, and a card
//! radio's file lands on removable media — only WHAT: the name has to end in an
//! extension the operation actually produces. That is enough to stop a call
//! with a wrong or hostile path from silently truncating something else, and it
//! costs a legitimate export nothing, because the save dialog already suggests
//! the right extension.
//!
//! Reads are not covered. They fail in their own parser on anything that is not
//! the format they expect, and refusing to *read* a file the operator picked
//! would break the legitimate case (an import whose name says nothing).

/// Refuse a write target whose extension is not one this operation produces.
///
/// `allowed` is lowercase and without dots, e.g. `&["csv"]`. Matching is
/// case-insensitive, so `CODEPLUG.CSV` from a Windows dialog passes.
pub(crate) fn check_write_target(path: &str, allowed: &[&str]) -> Result<(), String> {
    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase());

    let list = || {
        allowed
            .iter()
            .map(|e| format!(".{e}"))
            .collect::<Vec<_>>()
            .join(" or ")
    };

    match ext {
        Some(e) if allowed.contains(&e.as_str()) => Ok(()),
        Some(e) => Err(format!(
            "This export writes {}, but the file you chose ends in .{e}. \
             Pick a file name ending in {} — refusing to overwrite a file of another kind.",
            list(),
            list()
        )),
        None => Err(format!(
            "The file you chose has no extension. This export writes {}, so give the \
             name one — refusing to overwrite a file of another kind.",
            list()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything before a file's own test module — a `std::fs::write` inside
    /// `mod tests` is a fixture, not a command.
    fn production_half(src: &str) -> &str {
        match src.find("\n#[cfg(test)]") {
            Some(at) => &src[..at],
            None => src,
        }
    }

    /// One chunk per top-level `fn`, signature to next signature.
    fn top_level_fns(src: &str) -> Vec<(String, String)> {
        let src = production_half(src);
        let mut starts: Vec<usize> = Vec::new();
        for (nl, _) in src.match_indices('\n') {
            let rest = &src[nl + 1..];
            if ["fn ", "pub fn ", "async fn ", "pub async fn "]
                .iter()
                .any(|kw| rest.starts_with(kw))
            {
                starts.push(nl + 1);
            }
        }
        let mut out = Vec::new();
        for (n, &from) in starts.iter().enumerate() {
            let to = starts.get(n + 1).copied().unwrap_or(src.len());
            let body = &src[from..to];
            let name = body
                .split("fn ")
                .nth(1)
                .and_then(|s| s.split(['(', '<']).next())
                .unwrap_or("?")
                .to_string();
            out.push((name, body.to_string()));
        }
        out
    }

    /// A command that writes a file the OPERATOR named must check the name
    /// first.
    ///
    /// Crude on purpose, in the style of `radios/wiring.rs`: it reads the
    /// source for the two ways a command writes to a user-supplied `path` — a
    /// `std::fs::write(&path` and sqlite's `VACUUM INTO`, which additionally
    /// DELETES the target first — and requires the same function to mention
    /// the check. A fifth export added later is caught the day it is written.
    ///
    /// It keys on the identifier `&path` deliberately. The backup images
    /// written during a radio session go to `&backup_path` under the app data
    /// directory, which the operator never names and this rule does not touch.
    #[test]
    fn every_command_writing_an_operator_named_file_checks_the_name() {
        const WRITES: [&str; 2] = ["std::fs::write(&path", "VACUUM INTO"];
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands");
        let mut checked = 0;
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .expect("commands/ is readable")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "rs"))
            .collect();
        files.sort();
        for file in files {
            let src = std::fs::read_to_string(&file).expect("readable");
            for (name, body) in top_level_fns(&src) {
                let Some(marker) = WRITES.iter().find(|m| body.contains(**m)) else {
                    continue;
                };
                checked += 1;
                assert!(
                    body.contains("check_write_target"),
                    "{}: {name} writes to a path the operator named (`{marker}`) without \
                     calling check_write_target — so it will truncate whatever file that \
                     path points at, whatever kind of file it is.",
                    file.file_name().unwrap().to_string_lossy()
                );
            }
        }
        assert_eq!(
            checked, 5,
            "expected the five operator-named write paths (database, channels, talkgroups, \
             DMR users, codeplug) — found {checked}. If one moved, point this test at it \
             rather than leaving it passing vacuously"
        );
    }

    #[test]
    fn the_extension_this_export_writes_passes() {
        assert!(check_write_target("/Users/x/Documents/backup.sqlite3", &["sqlite3"]).is_ok());
        assert!(check_write_target("channels.json", &["json"]).is_ok());
        assert!(check_write_target("/Volumes/CARD/HOME.d75", &["d75"]).is_ok());
        // Several allowed, and a dialog that upper-cased it.
        assert!(check_write_target("PLUG.CSV", &["csv", "json"]).is_ok());
    }

    /// The point of the check: a path that is not what this command produces
    /// does not get truncated by it.
    #[test]
    fn another_kind_of_file_is_refused_by_name() {
        let err = check_write_target("/Users/x/.ssh/authorized_keys.pub", &["csv"]).unwrap_err();
        assert!(err.contains(".pub"), "{err}");
        assert!(err.contains(".csv"), "{err}");

        // The neighbouring export's own format is refused too — this is not
        // only about hostile paths.
        assert!(check_write_target("library.json", &["csv"]).is_err());
    }

    #[test]
    fn a_name_with_no_extension_at_all_is_refused() {
        let err = check_write_target("/Users/x/.ssh/authorized_keys", &["csv"]).unwrap_err();
        assert!(err.contains("no extension"), "{err}");
        assert!(err.contains(".csv"), "{err}");
    }

    /// A dotfile is a NAME, not an extension — `Path::extension` agrees, and
    /// this is the case where getting it wrong would be worst.
    #[test]
    fn a_dotfile_is_not_treated_as_having_that_extension() {
        assert!(check_write_target("/Users/x/.zshrc", &["zshrc"]).is_err());
        assert!(check_write_target("/Users/x/.env", &["env"]).is_err());
    }

    #[test]
    fn the_message_lists_every_extension_the_export_accepts() {
        let err = check_write_target("x.txt", &["csv", "json"]).unwrap_err();
        assert!(err.contains(".csv or .json"), "{err}");
    }
}
