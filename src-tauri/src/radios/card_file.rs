//! The file rule every card radio that patches ONE fixed file in place shares
//! (the FT5D's `BACKUP.dat`, the FTM-300's `CLNFTM300D.dat`): read the
//! operator's file, patch it, keep the untouched original as `.orig` — written
//! once and never overwritten, so a second write cannot clobber the only
//! pristine copy — and write the result back in place, because that fixed path
//! is what the radio's own restore menu reads.

/// `missing` is the sentence appended when the file cannot be read: which file
/// to pick, and how the radio writes it.
pub(crate) fn patch_in_place(
    path: &str,
    missing: &str,
    patch: impl FnOnce(&[u8]) -> Result<(Vec<u8>, usize), String>,
) -> Result<usize, String> {
    let template = std::fs::read(path).map_err(|e| format!("Could not read {path}: {e}. {missing}"))?;
    let (image, count) = patch(&template)?;
    let orig = format!("{path}.orig");
    if !std::path::Path::new(&orig).exists() {
        std::fs::write(&orig, &template)
            .map_err(|e| format!("Could not save the original as {orig}: {e}"))?;
    }
    std::fs::write(path, &image).map_err(|e| format!("Could not write {path}: {e}"))?;
    Ok(count)
}
