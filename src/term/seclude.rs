//! Secluded profiles: a profile's group runs its shells with HOME set to a
//! folder of its own, ~/.config/odek/homes/<profile>, so whatever a tool
//! keeps under home (logins in ~/.railway, ~/.config/gh, ~/.aws, kube
//! contexts, shell history) belongs to that profile alone.
//!
//! What every profile should see the same (shell setup, git and ssh,
//! toolchains, your folders, Claude Code) is linked in from the real home.
//! A link is made only where the folder has nothing yet, so a profile can
//! replace one with its own file, and `private` keeps one out entirely.
//! Folders panes start in are real paths, so they're the same either way.
//!
//! Logins kept in the macOS keychain rather than under home stay shared.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use super::profile::{self, Profile};

/// Linked from the real home into every profile's, where they exist.
pub const SHARED: &[&str] = &[
    // Shell setup.
    ".zshenv",
    ".zprofile",
    ".zshrc",
    ".zlogin",
    ".zlogout",
    ".zsh",
    ".oh-my-zsh",
    ".p10k.zsh",
    ".bashrc",
    ".bash_profile",
    ".profile",
    ".inputrc",
    // Git, ssh, editors.
    ".gitconfig",
    ".gitignore_global",
    ".config/git",
    ".ssh",
    ".gnupg",
    ".vimrc",
    ".vim",
    ".config/nvim",
    // Toolchains and what they've installed.
    ".cargo",
    ".rustup",
    "go",
    ".nvm",
    ".bun",
    ".deno",
    ".pyenv",
    ".rbenv",
    ".local",
    // odek's own settings and profiles, and Claude Code.
    ".config/odek",
    ".claude",
    ".claude.json",
    // The usual folders.
    "Applications",
    "Desktop",
    "Developer",
    "Documents",
    "Downloads",
    "Library",
    "Movies",
    "Music",
    "Pictures",
];

/// Where profiles' homes live: next to the profiles folder.
fn homes_folder() -> Option<PathBuf> {
    Some(profile::folder()?.parent()?.join("homes"))
}

/// The variables to set for `p`'s shells (none when it isn't secluded and
/// sets none). Makes its home and links on the way; a failure there leaves
/// the shells on the real home rather than half secluded.
pub fn env_for(p: &Profile) -> Vec<(OsString, OsString)> {
    let real = std::env::var_os("HOME").map(PathBuf::from);
    let (Some(real), Some(homes)) = (real, homes_folder()) else {
        return Vec::new();
    };
    match env_in(p, &real, &homes) {
        Ok(env) => env,
        Err(e) => {
            eprintln!("odek: could not set up the home for {}: {e}", p.name);
            own_env(p, &real, None)
        }
    }
}

/// The variables for the profile named `name`, read from its file now.
pub fn env_named(name: &str) -> Vec<(OsString, OsString)> {
    let Some(dir) = profile::folder() else {
        return Vec::new();
    };
    profile::load_all(&dir)
        .0
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(name))
        .map(|p| env_for(&p))
        .unwrap_or_default()
}

fn env_in(p: &Profile, real: &Path, homes: &Path) -> io::Result<Vec<(OsString, OsString)>> {
    if !p.home.seclude {
        return Ok(own_env(p, real, None));
    }
    let home = homes.join(folder_name(&p.name));
    std::fs::create_dir_all(&home)?;
    let shared = SHARED
        .iter()
        .map(|s| s.to_string())
        .chain(p.home.share.iter().cloned())
        .filter(|s| !p.home.private.iter().any(|k| k == s));
    for rel in shared {
        link(real, &home, &rel)?;
    }
    // A private entry still linked from before: unlink it (links only).
    for rel in &p.home.private {
        let at = home.join(rel);
        if at.symlink_metadata().is_ok_and(|m| m.is_symlink())
            && std::fs::read_link(&at).is_ok_and(|to| to == real.join(rel))
        {
            std::fs::remove_file(&at)?;
        }
    }
    let mut env = vec![
        (OsString::from("HOME"), home.clone().into_os_string()),
        (OsString::from("ODEK_REAL_HOME"), real.as_os_str().to_owned()),
    ];
    // XDG folders set to somewhere under the real home move with it.
    for key in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ] {
        if let Some(rest) = std::env::var_os(key)
            .and_then(|v| PathBuf::from(v).strip_prefix(real).ok().map(Path::to_path_buf))
        {
            env.push((key.into(), home.join(rest).into_os_string()));
        }
    }
    env.extend(own_env(p, real, Some(&home)));
    Ok(env)
}

/// The profile's name and `[env]`, with `~/` as the home its shells get.
fn own_env(p: &Profile, real: &Path, home: Option<&Path>) -> Vec<(OsString, OsString)> {
    let home = home.unwrap_or(real);
    let mut env = vec![(OsString::from("ODEK_PROFILE"), OsString::from(&p.name))];
    for (k, v) in &p.home.env {
        let v = match v.strip_prefix("~/") {
            Some(rest) => home.join(rest).into_os_string(),
            None if v == "~" => home.as_os_str().to_owned(),
            None => v.into(),
        };
        env.push((k.into(), v));
    }
    env
}

/// Link `rel` in `home` to the real one, if that exists and `home` has
/// nothing there yet.
fn link(real: &Path, home: &Path, rel: &str) -> io::Result<()> {
    let from = real.join(rel);
    let at = home.join(rel);
    if from.symlink_metadata().is_err() || at.symlink_metadata().is_ok() {
        return Ok(());
    }
    if let Some(parent) = at.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::os::unix::fs::symlink(&from, &at)
}

/// A profile's name as a folder name.
fn folder_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c == '/' || c == ':' || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let s = s.trim().trim_start_matches('.');
    if s.is_empty() { "profile".into() } else { s.into() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::profile::Home;

    fn profile(home: Home) -> Profile {
        Profile {
            name: "Work".into(),
            open_at_launch: false,
            tabs: Vec::new(),
            home,
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("odek-seclude-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn get<'a>(env: &'a [(OsString, OsString)], key: &str) -> Option<&'a OsString> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    #[test]
    fn gives_a_home_of_its_own_with_shared_things_linked() {
        let root = tmp("home");
        let real = root.join("real");
        std::fs::create_dir_all(real.join(".ssh")).unwrap();
        std::fs::create_dir_all(real.join(".aws")).unwrap();
        std::fs::create_dir_all(real.join(".railway")).unwrap();
        std::fs::write(real.join(".zshrc"), "").unwrap();
        std::fs::write(real.join(".claude.json"), "").unwrap();
        let p = profile(Home {
            share: vec![".aws".into()],
            private: vec![".claude.json".into()],
            env: vec![("KUBECONFIG".into(), "~/.kube/work".into())],
            ..Home::default()
        });
        let env = env_in(&p, &real, &root.join("homes")).unwrap();
        let home = root.join("homes/Work");
        assert_eq!(get(&env, "HOME"), Some(&home.clone().into_os_string()));
        assert_eq!(get(&env, "ODEK_REAL_HOME"), Some(&real.clone().into_os_string()));
        assert_eq!(get(&env, "ODEK_PROFILE"), Some(&"Work".into()));
        assert_eq!(
            get(&env, "KUBECONFIG"),
            Some(&home.join(".kube/work").into_os_string())
        );
        for shared in [".zshrc", ".ssh", ".aws"] {
            assert_eq!(std::fs::read_link(home.join(shared)).unwrap(), real.join(shared));
        }
        // Logins and anything not listed stay apart.
        assert!(home.join(".railway").symlink_metadata().is_err());
        assert!(home.join(".claude.json").symlink_metadata().is_err());

        // A profile's own file is never replaced by a link…
        std::fs::remove_file(home.join(".zshrc")).unwrap();
        std::fs::write(home.join(".zshrc"), "mine").unwrap();
        // …and making something private later unlinks it.
        let p = profile(Home {
            private: vec![".ssh".into()],
            ..Home::default()
        });
        env_in(&p, &real, &root.join("homes")).unwrap();
        assert_eq!(std::fs::read_to_string(home.join(".zshrc")).unwrap(), "mine");
        assert!(home.join(".ssh").symlink_metadata().is_err());
        assert!(real.join(".ssh").is_dir());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn not_secluded_keeps_the_real_home() {
        let root = tmp("off");
        let p = profile(Home {
            seclude: false,
            env: vec![("A".into(), "~".into())],
            ..Home::default()
        });
        let env = env_in(&p, &root, &root.join("homes")).unwrap();
        assert_eq!(get(&env, "HOME"), None);
        assert_eq!(get(&env, "A"), Some(&root.clone().into_os_string()));
        assert!(!root.join("homes").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn folder_names() {
        assert_eq!(folder_name("Work: client/a"), "Work- client-a");
        assert_eq!(folder_name("..."), "profile");
    }
}
