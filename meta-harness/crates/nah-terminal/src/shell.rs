//! Starting a shell: the user's own, in the folder asked for, set up to
//! mark where its commands start and end (bash and zsh; others start as
//! they are). The user's own start-up files still run first.

use std::path::{Path, PathBuf};

use portable_pty::CommandBuilder;

/// The shell to run: `$SHELL`, else `/bin/sh`.
pub fn users_shell() -> String {
    std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".to_owned())
}

/// The command that starts `shell` in `folder`, its marks set up with
/// files kept in `hooks` (written there as needed).
pub fn command(shell: &str, folder: &Path, hooks: &Path) -> std::io::Result<CommandBuilder> {
    let mut cmd = CommandBuilder::new(shell);
    cmd.cwd(folder);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    cmd.env("TERM_PROGRAM", "nah");
    match Path::new(shell).file_name().and_then(|n| n.to_str()) {
        Some("bash") => {
            let rc = write(hooks, "bashrc", BASH)?;
            cmd.arg("--rcfile");
            cmd.arg(rc);
            cmd.arg("-i");
        }
        Some("zsh") => {
            let dir = hooks.join("zsh");
            std::fs::create_dir_all(&dir)?;
            write(&dir, ".zshenv", ZSHENV)?;
            write(&dir, ".zshrc", ZSHRC)?;
            // Its own start-up files are read from where they were.
            let theirs = std::env::var("ZDOTDIR").unwrap_or_else(|_| std::env::var("HOME").unwrap_or_default());
            cmd.env("NAH_USER_ZDOTDIR", theirs);
            cmd.env("ZDOTDIR", dir);
        }
        _ => {}
    }
    Ok(cmd)
}

fn write(dir: &Path, name: &str, text: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(name);
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text) {
        std::fs::write(&path, text)?;
    }
    Ok(path)
}

/// bash: the user's `.bashrc`, then a mark before each command runs (`PS0`)
/// and one with its code before each prompt (`PROMPT_COMMAND`, first).
const BASH: &str = r#"[ -f ~/.bashrc ] && . ~/.bashrc
__nah_done() { local code=$?; printf '\e]133;D;%s\a' "$code"; return $code; }
PROMPT_COMMAND="__nah_done${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
PS0=$'\e]133;C\a'"${PS0}"
"#;

/// zsh reads `.zshenv` then `.zshrc` from `ZDOTDIR`: each reads the user's
/// own (where their `.zshenv` says, if it moves `ZDOTDIR`), then `.zshrc`
/// adds the marks and leaves `ZDOTDIR` the user's.
const ZSHENV: &str = r#"__nah_hooks="$ZDOTDIR"
ZDOTDIR="$NAH_USER_ZDOTDIR"
[ -f "$ZDOTDIR/.zshenv" ] && . "$ZDOTDIR/.zshenv"
NAH_USER_ZDOTDIR="$ZDOTDIR"
ZDOTDIR="$__nah_hooks"
unset __nah_hooks
"#;

const ZSHRC: &str = r#"ZDOTDIR="$NAH_USER_ZDOTDIR"
[ -f "$ZDOTDIR/.zshrc" ] && . "$ZDOTDIR/.zshrc"
__nah_precmd() { local code=$?; printf '\e]133;D;%s\a' "$code"; }
__nah_preexec() { printf '\e]133;C\a'; }
precmd_functions=(__nah_precmd $precmd_functions)
preexec_functions+=(__nah_preexec)
"#;
