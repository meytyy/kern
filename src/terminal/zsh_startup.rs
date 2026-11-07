//! Shell completion for zsh users whose configuration never loads it.
//!
//! zsh completes command arguments only after `compinit`, and macOS does not call
//! it by default. KERN cannot change the user's dotfiles, so interactive zsh starts
//! through thin copies that source the real files in the usual order, and run
//! `compinit` last, only when those files have not loaded it. `ZDOTDIR` goes back
//! to the real value before the prompt, so nested shells see nothing unusual.
use crate::core::settings::config_dir;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const ZSHENV: &str = r#"KERN_STARTUP_DIR=$ZDOTDIR
ZDOTDIR=${KERN_USER_ZDOTDIR:-$HOME}
[[ -r $ZDOTDIR/.zshenv ]] && source $ZDOTDIR/.zshenv
KERN_USER_ZDOTDIR=$ZDOTDIR
ZDOTDIR=$KERN_STARTUP_DIR
"#;

const ZPROFILE: &str = r#"ZDOTDIR=$KERN_USER_ZDOTDIR
[[ -r $ZDOTDIR/.zprofile ]] && source $ZDOTDIR/.zprofile
KERN_USER_ZDOTDIR=$ZDOTDIR
ZDOTDIR=$KERN_STARTUP_DIR
"#;

// /etc/zshrc has already derived HISTFILE from the temporary ZDOTDIR.
const ZSHRC: &str = r#"[[ $HISTFILE == $KERN_STARTUP_DIR/* ]] && HISTFILE=$KERN_USER_ZDOTDIR/${HISTFILE:t}
ZDOTDIR=$KERN_USER_ZDOTDIR
[[ -r $ZDOTDIR/.zshrc ]] && source $ZDOTDIR/.zshrc
if [[ -o interactive ]] && (( ! $+functions[compdef] )); then
  autoload -Uz compinit && compinit -i -d "$KERN_STARTUP_DIR/.zcompdump"
fi
KERN_USER_ZDOTDIR=$ZDOTDIR
ZDOTDIR=$KERN_STARTUP_DIR
"#;

const ZLOGIN: &str = r#"ZDOTDIR=$KERN_USER_ZDOTDIR
[[ -r $ZDOTDIR/.zlogin ]] && source $ZDOTDIR/.zlogin
unset KERN_USER_ZDOTDIR KERN_STARTUP_DIR
if [[ $ZDOTDIR == $HOME ]]; then unset ZDOTDIR; fi
"#;

fn write_files(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, body) in [
        (".zshenv", ZSHENV),
        (".zprofile", ZPROFILE),
        (".zshrc", ZSHRC),
        (".zlogin", ZLOGIN),
    ] {
        std::fs::write(dir.join(name), body)?;
    }
    Ok(())
}

fn startup_dir() -> PathBuf {
    config_dir().join("zsh-startup")
}

/// On any failure the shell simply starts the normal way.
pub fn apply(env: &mut HashMap<String, String>) {
    let dir = startup_dir();
    if write_files(&dir).is_err() {
        return;
    }
    if let Some(user) = env.get("ZDOTDIR").cloned() {
        env.insert("KERN_USER_ZDOTDIR".into(), user);
    }
    env.insert("ZDOTDIR".into(), dir.to_string_lossy().into_owned());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Runs a real login+interactive zsh with a fake home.
    fn run(home: &Path, startup: &Path, script: &str) -> String {
        write_files(startup).unwrap();
        let out = Command::new("/bin/zsh")
            .args(["-l", "-i", "-c", script])
            .env_clear()
            .env("HOME", home)
            .env("PATH", "/usr/bin:/bin")
            .env("TERM", "dumb")
            .env("ZDOTDIR", startup)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn user_files_run_in_order_and_nothing_is_left_behind() {
        let home = tempfile::tempdir().unwrap();
        let startup = tempfile::tempdir().unwrap();
        for f in [".zshenv", ".zprofile", ".zshrc", ".zlogin"] {
            std::fs::write(
                home.path().join(f),
                format!("print -r -- loaded{f} ${{ZDOTDIR-unset}}\n"),
            )
            .unwrap();
        }
        let out = run(
            home.path(),
            startup.path(),
            "print -r -- final ${ZDOTDIR-unset} ${KERN_STARTUP_DIR-gone} $+functions[compdef] $HISTFILE",
        );
        let h = home.path().to_string_lossy();
        let order: Vec<&str> = out
            .lines()
            .filter_map(|l| l.split(' ').next())
            .filter(|w| w.starts_with("loaded"))
            .collect();
        assert_eq!(
            order,
            [
                "loaded.zshenv",
                "loaded.zprofile",
                "loaded.zshrc",
                "loaded.zlogin"
            ],
            "{out}"
        );
        assert!(out.contains(&format!("loaded.zshrc {h}")), "{out}");
        assert!(
            out.contains(&format!("final unset gone 1 {h}/.zsh_history")),
            "{out}"
        );
    }

    #[test]
    fn a_users_own_compinit_is_left_alone() {
        let home = tempfile::tempdir().unwrap();
        let startup = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join(".zshrc"),
            "autoload -Uz compinit; compinit -i -d $HOME/.mydump\n",
        )
        .unwrap();
        run(home.path(), startup.path(), "true");
        assert!(home.path().join(".mydump").exists());
        assert!(!startup.path().join(".zcompdump").exists());
    }
}
