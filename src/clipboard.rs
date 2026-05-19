use anyhow::{anyhow, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

/// Copy `text` to the system clipboard. Tries pbcopy (macOS), wl-copy
/// (Linux/Wayland), then xclip (Linux/X11). Returns an error listing what was
/// tried if none are available.
pub fn copy(text: &str) -> Result<&'static str> {
    let candidates: &[(&str, &[&str])] = &[
        ("pbcopy", &[]),
        ("wl-copy", &[]),
        ("xclip", &["-selection", "clipboard"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (cmd, args) in candidates {
        match try_pipe(cmd, args, text) {
            Ok(()) => return Ok(cmd),
            Err(e) if is_not_found(&e) => continue,
            Err(e) => return Err(e).with_context(|| format!("{} failed", cmd)),
        }
    }
    Err(anyhow!(
        "no clipboard tool found (tried pbcopy, wl-copy, xclip, xsel)"
    ))
}

fn try_pipe(cmd: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| anyhow!("no stdin on {}", cmd))?
        .write_all(text.as_bytes())?;
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("{} exited with {}", cmd, status);
    }
    Ok(())
}

fn is_not_found(err: &anyhow::Error) -> bool {
    err.downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}
