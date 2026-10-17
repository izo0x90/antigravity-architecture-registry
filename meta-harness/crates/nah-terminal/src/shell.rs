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

/// bash: the user's `.bashrc`, then the marks: one with the last
/// command's code before each prompt (`PROMPT_COMMAND`, first), the prompt
/// marked where it starts and where typing starts (wrapped around `PS1`,
/// last, after the user's own made it), and before each command runs
/// (`PS0`) the command line (the history's newest, when it took one) and
/// a mark. bash 5.1 takes `PROMPT_COMMAND` as a list: the user's stay as
/// they are; older ones get one line (the user's last `;` taken off).
const BASH: &str = r#"[ -f ~/.bashrc ] && . ~/.bashrc
__nah_done() { local code=$?; printf '\e]133;D;%s\a' "$code"; return $code; }
__nah_prompt() {
  local code=$?
  [[ $PS1 == *'133;A'* ]] || PS1=$'\[\e]133;A\a\]'"$PS1"$'\[\e]133;B\a\]'
  __nah_newest=$(HISTTIMEFORMAT= builtin history 1)
  return $code
}
__nah_escape() { local s=${1//\\/\\\\}; s=${s//;/\\x3b}; s=${s//$'\n'/\\x0a}; s=${s//$'\r'/\\x0d}; s=${s//$'\e'/\\x1b}; s=${s//$'\a'/\\x07}; printf '%s' "$s"; }
__nah_typed() {
  local newest
  newest=$(HISTTIMEFORMAT= builtin history 1)
  [[ $newest != "$__nah_newest" && $newest =~ ^[[:space:]]*[0-9]+[*[:space:]][[:space:]]*(.*)$ ]] && printf '\e]633;E;%s\a' "$(__nah_escape "${BASH_REMATCH[1]}")"
}
if (( BASH_VERSINFO[0] > 5 || (BASH_VERSINFO[0] == 5 && BASH_VERSINFO[1] >= 1) )); then
  PROMPT_COMMAND=(__nah_done "${PROMPT_COMMAND[@]}" __nah_prompt)
else
  __nah_theirs=${PROMPT_COMMAND%"${PROMPT_COMMAND##*[![:space:];]}"}
  PROMPT_COMMAND="__nah_done${__nah_theirs:+;$__nah_theirs};__nah_prompt"
fi
PS0='$(__nah_typed)'$'\e]133;C\a'"${PS0}"
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
__nah_prompt() { [[ $PS1 == *'133;A'* ]] || PS1=$'%{\e]133;A\a%}'"$PS1"$'%{\e]133;B\a%}'; }
__nah_escape() { local s=${1//\\/\\\\}; s=${s//;/\\x3b}; s=${s//$'\n'/\\x0a}; s=${s//$'\r'/\\x0d}; s=${s//$'\e'/\\x1b}; s=${s//$'\a'/\\x07}; printf '%s' "$s"; }
__nah_preexec() { printf '\e]633;E;%s\a\e]133;C\a' "$(__nah_escape "$1")"; }
precmd_functions=(__nah_precmd $precmd_functions __nah_prompt)
preexec_functions+=(__nah_preexec)
"#;

#[cfg(test)]
mod tests {
    use super::*;

    /// bash with the user's start-up file saying `theirs`: what each
    /// prompt runs, run; what it wrote on its error stream.
    fn prompt_errors(theirs: &str) -> String {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join(".bashrc"), theirs).unwrap();
        let rc = write(home.path(), "nah-bashrc", BASH).unwrap();
        let run = r#". "$1"; for c in "${PROMPT_COMMAND[@]}"; do eval "$c"; done; [[ $PS1 == *'133;A'* ]] || echo "no prompt marks" >&2"#;
        let out = std::process::Command::new("bash").args(["-c", run, "bash"]).arg(&rc).env("HOME", home.path()).output().unwrap();
        String::from_utf8_lossy(&out.stderr).into_owned()
    }

    #[test]
    fn bash_keeps_the_users_prompt_command_whatever_it_ends_with() {
        for theirs in ["", "PROMPT_COMMAND='history -a; '", "PROMPT_COMMAND='history -a;'", "PROMPT_COMMAND=(': one' 'PS1=\"x$ \"')"] {
            assert_eq!(prompt_errors(theirs), "", "{theirs}");
        }
    }
}
