use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    pub tool: String,
    pub path: String,
}

fn outside(prefix: &Path, candidate: &Path) -> bool {
    let Ok(real) = candidate.canonicalize() else { return false };
    let home = prefix.canonicalize().unwrap_or_else(|_| prefix.to_path_buf());
    real.is_file() && !real.starts_with(home)
}

pub fn find(prefix: &Path, tool: &str, binaries: &[String], dirs: &[PathBuf]) -> Vec<Shadow> {
    dirs.iter()
        .flat_map(|dir| binaries.iter().map(move |binary| dir.join(binary)))
        .filter(|candidate| outside(prefix, candidate))
        .map(|candidate| Shadow { tool: tool.to_string(), path: candidate.display().to_string() })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sth-update-shadow-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("brew/bin")).unwrap();
        fs::create_dir_all(dir.join("local")).unwrap();
        fs::write(dir.join("brew/bin/nudge"), "brew").unwrap();
        dir
    }

    #[test]
    fn a_copy_outside_brew_shadows_the_tool() {
        let dir = scratch("copy");
        fs::write(dir.join("local/nudge"), "dev build").unwrap();
        let found = find(&dir.join("brew"), "nudge", &["nudge".to_string()], &[dir.join("local")]);
        assert_eq!(found, vec![Shadow { tool: "nudge".into(), path: dir.join("local/nudge").display().to_string() }]);
    }

    #[test]
    fn a_link_back_into_brew_is_not_a_shadow() {
        let dir = scratch("link");
        std::os::unix::fs::symlink(dir.join("brew/bin/nudge"), dir.join("local/nudge")).unwrap();
        assert!(find(&dir.join("brew"), "nudge", &["nudge".to_string()], &[dir.join("local")]).is_empty());
    }

    #[test]
    fn nothing_in_the_way_means_no_shadow() {
        let dir = scratch("none");
        assert!(
            find(&dir.join("brew"), "nudge", &["nudge".to_string()], &[dir.join("local"), dir.join("missing")])
                .is_empty()
        );
    }
}
