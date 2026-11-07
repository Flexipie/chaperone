//! A child process with a deadline: `daisy-cli reviews get` and the git
//! calls of `overlap` and `impact`.

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

pub struct Done {
    pub status: ExitStatus,
    /// At most the cap's bytes of stdout.
    pub out: Vec<u8>,
    /// Stdout went past the cap.
    pub cut: bool,
    pub err: Vec<u8>,
}

pub enum Failed {
    Spawn(std::io::Error),
    TimedOut,
    Wait(std::io::Error),
}

/// Runs `cmd` with stdin closed, killing it at `deadline`. Both pipes are
/// read to the end while it runs, so a large answer cannot fill one and
/// stall the child until the deadline; stdout past `cap` bytes is dropped.
pub fn run(cmd: &mut Command, deadline: Instant, cap: usize) -> Result<Done, Failed> {
    if Instant::now() >= deadline {
        return Err(Failed::TimedOut);
    }
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(Failed::Spawn)?;
    let drain = |pipe: Option<Box<dyn Read + Send>>, cap: usize| {
        std::thread::spawn(move || {
            let (mut buf, mut cut) = (Vec::new(), false);
            if let Some(mut pipe) = pipe {
                let mut chunk = [0u8; 8192];
                loop {
                    let n = match pipe.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    };
                    let keep = n.min(cap - buf.len());
                    buf.extend_from_slice(&chunk[..keep]);
                    cut |= keep < n;
                }
            }
            (buf, cut)
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as _), cap);
    let err = drain(child.stderr.take().map(|p| Box::new(p) as _), usize::MAX);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failed::TimedOut);
            }
            Err(e) => return Err(Failed::Wait(e)),
        }
    };
    let (out, cut) = out.join().unwrap_or_default();
    let (err, _) = err.join().unwrap_or_default();
    Ok(Done {
        status,
        out,
        cut,
        err,
    })
}
