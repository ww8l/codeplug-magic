//! The `.icf` card-file rules the Icom card radios share (ID-52 #38, ID-51
//! Plus #50): which file in the radio's `Setting` folder is the template, what
//! a new file is called, and how a write patches without touching anything the
//! operator saved.
//!
//! The radio names its own saves `SetYYYYMMDD_NN.icf` and lists them on its
//! Load Setting screen, so a write into the folder creates the next free name
//! from the newest usable save there; a write to an existing file patches it in
//! place and keeps the untouched original once as `.orig`.

use std::path::{Path, PathBuf};

use crate::radios::icom_id52::icf::IcfFile;

/// One radio's card folder: how it is named to the operator, and which files in
/// it this driver can patch.
pub(crate) struct IcfCard {
    /// "ID-52", "ID-51 Plus".
    pub radio: &'static str,
    /// The folder on the card, for messages: "ID-52/Setting/".
    pub folder: &'static str,
    /// The radio's own menu path for saving one: "SET > SD Card > Save Setting".
    pub save_menu: &'static str,
    /// Whether a parsed file is one this driver's offsets describe.
    pub is_ours: fn(&IcfFile) -> bool,
}

impl IcfCard {
    /// The template a new file is built from: the newest settings file in
    /// `dir` that this driver can patch. The radio's names sort by date, so the
    /// files are tried newest first and the first usable one wins — one parse in
    /// the usual case, rather than every file on the card.
    pub(crate) fn newest_settings_file(&self, dir: &Path) -> Result<String, String> {
        let mut names: Vec<PathBuf> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("icf")))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
            .iter()
            .rev()
            .find(|p| {
                std::fs::read_to_string(p).is_ok_and(|t| IcfFile::parse(&t).is_ok_and(|f| (self.is_ours)(&f)))
            })
            .map(|p| p.to_string_lossy().into_owned())
            .ok_or_else(|| {
                format!(
                    "No {} settings file to work from in {}. On the radio, save one with {}, \
                     then try again.",
                    self.radio,
                    dir.display(),
                    self.save_menu
                )
            })
    }

    /// The `.icf` file rule, shared by the export and the settings-only write.
    ///
    /// A target that does not exist yet is a NEW file on the card, and the
    /// template is the newest usable settings file beside it. A target that does
    /// exist is the operator pointing at one file and meaning it: it is patched
    /// in place, with the untouched original kept once as `.orig` (written
    /// once and never overwritten, so a second export cannot clobber the only
    /// pristine copy).
    pub(crate) fn patch_icf<T>(
        &self,
        path: &str,
        patch: impl FnOnce(&mut IcfFile) -> Result<T, String>,
    ) -> Result<T, String> {
        let template = if Path::new(path).is_file() {
            path.to_string()
        } else {
            let dir = Path::new(path).parent().ok_or("There is no folder to write into.")?;
            self.newest_settings_file(dir)?
        };
        let text = std::fs::read_to_string(&template).map_err(|e| {
            format!(
                "Could not read {template}: {e}. Pick a settings file the radio saved for itself \
                 with {} — they live in {}.",
                self.save_menu, self.folder
            )
        })?;
        let mut icf = IcfFile::parse(&text)?;
        let out = patch(&mut icf)?;
        // Only when overwriting the operator's own file: a NEW file leaves
        // theirs untouched by construction.
        if template == path {
            let orig = format!("{path}.orig");
            if !Path::new(&orig).exists() {
                std::fs::write(&orig, &text)
                    .map_err(|e| format!("Could not save the original as {orig}: {e}"))?;
            }
        }
        std::fs::write(path, icf.render()).map_err(|e| format!("Could not write {path}: {e}"))?;
        Ok(out)
    }

    /// What the exporter writes for a picked target: a folder means "make me a
    /// new file in here"; anything else is written where it points.
    pub(crate) fn resolve_target(&self, path: &str) -> Result<String, String> {
        let dir = Path::new(path);
        if !dir.is_dir() {
            return Ok(path.to_string());
        }
        // Prove there is something to patch before naming the output.
        self.newest_settings_file(dir)?;
        Ok(next_settings_file(dir).to_string_lossy().into_owned())
    }
}

/// The radios' own naming scheme, `SetYYYYMMDD_NN.icf`, with today's date and
/// the first free number. 14 characters before the extension: inside both the
/// ID-52's picker limit (23) and the ID-51's (16).
pub(crate) fn next_settings_file(dir: &Path) -> PathBuf {
    let today = chrono::Local::now().format("%Y%m%d").to_string();
    for n in 1..=99 {
        let candidate = dir.join(format!("Set{today}_{n:02}.icf"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(format!("Set{today}_99.icf"))
}
