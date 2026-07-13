/// Tauri commands return `Result<T, String>` so the frontend can surface
/// errors via toast notifications. This extension lets us convert any
/// `Result<T, E: Display>` (sqlx errors, IO errors, serde errors, …) into
/// `Result<T, String>` ergonomically with `.estr()`.
pub trait MapErrString<T> {
    fn estr(self) -> Result<T, String>;
}

impl<T, E: std::fmt::Display> MapErrString<T> for Result<T, E> {
    fn estr(self) -> Result<T, String> {
        self.map_err(|e| e.to_string())
    }
}
