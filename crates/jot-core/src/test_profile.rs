//! Explicit, portable test profiles. Checked before any application service starts.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn root_for(executable: &Path, enabled: bool) -> Option<PathBuf> {
    let directory = executable.parent()?;
    (enabled || directory.join("jot-test-profile").is_file()).then(|| directory.join("profile"))
}

/// A marker beside the portable executable also isolates direct double-clicks.
/// Fail closed if an explicitly requested profile cannot resolve its executable.
pub fn root() -> Option<&'static Path> {
    static ROOT: OnceLock<Option<PathBuf>> = OnceLock::new();
    ROOT.get_or_init(|| {
        let exe = std::env::current_exe().expect("cannot resolve executable for profile isolation");
        root_for(
            &exe,
            std::env::var_os("JOT_TEST_PROFILE").is_some_and(|v| v == "1"),
        )
    })
    .as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn marker_and_environment_isolate_but_normal_installation_keeps_its_profile() {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("jot.exe");
        let expected = Some(temp.path().join("profile"));
        assert_eq!(root_for(&exe, false), None);
        assert_eq!(root_for(&exe, true), expected);
        std::fs::write(temp.path().join("jot-test-profile"), "").unwrap();
        assert_eq!(root_for(&exe, false), expected);
    }
}
