//! Starting a desktop entry on a file: its `Exec` value split into arguments, with the field codes
//! filled in.

use std::path::Path;

use super::entry::DesktopEntry;

/// Why an `Exec` value gives no command line.
#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("the Exec value ends inside an escape: {0}")]
    EndsInEscape(String),
    #[error("the Exec value has an unclosed quote: {0}")]
    UnclosedQuote(String),
    #[error("{id} has no Exec key")]
    NoExec { id: String },
    #[error("{id} uses the unknown field code %{code}")]
    UnknownFieldCode { id: String, code: char },
    #[error("{id} ends its Exec value with %")]
    EndsWithPercent { id: String },
    #[error("{id} has an empty Exec value")]
    EmptyExec { id: String },
}

/// Why a desktop entry did not start.
#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("{id} runs in a terminal")]
    Terminal { id: String },
    #[error("{0}")]
    CommandLine(#[from] ExecError),
    #[error("{program} did not start: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
}

/// The program of an `Exec` value: its first argument.
pub(super) fn exec_program(exec: &str) -> Option<String> {
    split_exec(exec).ok()?.into_iter().next().map(|arg| arg.text)
}
/// One argument of an `Exec` value and whether it was quoted (field codes are not expanded in quoted
/// arguments).
#[derive(Debug, Clone, PartialEq)]
struct ExecArg {
    text: String,
    quoted: bool,
}

/// The arguments of an `Exec` value (string escapes already undone): split at unquoted spaces, a
/// double-quoted argument taking `\"`, `` \` ``, `\$` and `\\` as escapes.
fn split_exec(exec: &str) -> Result<Vec<ExecArg>, ExecError> {
    let mut args = Vec::new();
    let mut current: Option<ExecArg> = None;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' | '\n' => args.extend(current.take()),
            '"' => {
                let arg = current.get_or_insert(ExecArg { text: String::new(), quoted: true });
                arg.quoted = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(escaped @ ('"' | '`' | '$' | '\\')) => arg.text.push(escaped),
                            Some(other) => {
                                arg.text.push('\\');
                                arg.text.push(other);
                            }
                            None => return Err(ExecError::EndsInEscape(exec.to_owned())),
                        },
                        Some(other) => arg.text.push(other),
                        None => return Err(ExecError::UnclosedQuote(exec.to_owned())),
                    }
                }
            }
            _ => current.get_or_insert(ExecArg { text: String::new(), quoted: false }).text.push(c),
        }
    }

    args.extend(current);
    Ok(args)
}

/// The command line that starts `entry` on `file`: its `Exec` arguments with the field codes filled
/// in (`%f` `%F` the path, `%u` `%U` its `file://` URL, `%i` the icon, `%c` the name, `%k` the desktop
/// file; the deprecated codes dropped).
pub fn command_line(entry: &DesktopEntry, file: &Path, file_url: &str) -> Result<Vec<String>, ExecError> {
    let exec = entry.exec.as_deref().ok_or_else(|| ExecError::NoExec { id: entry.id.clone() })?;
    let path = file.to_string_lossy().into_owned();
    let mut line = Vec::new();
    for arg in split_exec(exec)? {
        if arg.quoted {
            line.push(arg.text);
            continue;
        }
        match arg.text.as_str() {
            "%F" => line.push(path.clone()),
            "%U" => line.push(file_url.to_owned()),
            "%i" => {
                if let Some(icon) = &entry.icon {
                    line.extend(["--icon".to_owned(), icon.clone()]);
                }
            }
            _ => {
                let mut text = String::new();
                let mut chars = arg.text.chars();
                while let Some(c) = chars.next() {
                    if c != '%' {
                        text.push(c);
                        continue;
                    }
                    match chars.next() {
                        Some('f' | 'F') => text.push_str(&path),
                        Some('u' | 'U') => text.push_str(file_url),
                        Some('c') => text.push_str(&entry.name),
                        Some('k') => text.push_str(&entry.path.to_string_lossy()),
                        Some('%') => text.push('%'),
                        Some('d' | 'D' | 'n' | 'N' | 'v' | 'm' | 'i') => {}
                        Some(other) => return Err(ExecError::UnknownFieldCode { id: entry.id.clone(), code: other }),
                        None => return Err(ExecError::EndsWithPercent { id: entry.id.clone() }),
                    }
                }
                if !text.is_empty() {
                    line.push(text);
                }
            }
        }
    }

    if line.is_empty() {
        return Err(ExecError::EmptyExec { id: entry.id.clone() });
    }
    Ok(line)
}

/// Starts `entry` on `file` and returns once it started; the process is reaped when it ends.
pub fn launch(entry: &DesktopEntry, file: &Path, file_url: &str) -> Result<(), LaunchError> {
    if entry.terminal {
        return Err(LaunchError::Terminal { id: entry.id.clone() });
    }

    let line = command_line(entry, file, file_url)?;
    let mut command = std::process::Command::new(&line[0]);
    command
        .args(&line[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(dir) = &entry.working_dir {
        command.current_dir(dir);
    }

    let mut child = command.spawn().map_err(|source| LaunchError::Spawn { program: line[0].clone(), source })?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
}
