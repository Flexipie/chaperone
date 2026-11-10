//! A child process with a deadline: `daisy-cli reviews get`, the UI
//! backlog's daisy reads, the UI writer's gh and herdr calls and the git
//! calls of `overlap`, `impact` and the UI's diff.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
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
    start(cmd, None, false, deadline, cap)
}

/// [`run`] for an agent session, with `input` written to the child's
/// stdin, which is then closed: a prompt too long for an argument. A
/// child that exits before it has read all of it is a [`Failed::Wait`],
/// whatever it printed. The child leads a process group of its own and at
/// the deadline the whole group is killed, so a shell or a build the
/// agent started does not outlive it and hold the pipes open. The cost:
/// a Ctrl-C at the terminal no longer reaches the session.
pub fn run_input(
    cmd: &mut Command,
    input: Option<&[u8]>,
    deadline: Instant,
    cap: usize,
) -> Result<Done, Failed> {
    start(cmd, input, true, deadline, cap)
}

fn start(
    cmd: &mut Command,
    input: Option<&[u8]>,
    group: bool,
    deadline: Instant,
    cap: usize,
) -> Result<Done, Failed> {
    if Instant::now() >= deadline {
        return Err(Failed::TimedOut);
    }
    if group {
        cmd.process_group(0);
    }
    let mut child = cmd
        .stdin(match input {
            Some(_) => Stdio::piped(),
            None => Stdio::null(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(Failed::Spawn)?;
    // From a thread, as the pipes below are read: a child that answers
    // before it has read everything would stall both sides.
    let (sent, written) = std::sync::mpsc::channel();
    if let (Some(mut pipe), Some(input)) = (child.stdin.take(), input.map(<[u8]>::to_vec)) {
        std::thread::spawn(move || sent.send(pipe.write_all(&input)));
    }
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
                // std has no killpg and `unsafe` is forbidden, so the
                // system's kill sends it. The child is not reaped yet, so
                // its pid still names its own group and no other.
                if group {
                    let _ = Command::new("/bin/kill")
                        .args(["-KILL", "--", &format!("-{}", child.id())])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failed::TimedOut);
            }
            Err(e) => return Err(Failed::Wait(e)),
        }
    };
    // An answer to half the input is none. The write has ended unless
    // something the child started still holds the pipe; then the deadline
    // ends the wait.
    if input.is_some() {
        let left = deadline.saturating_duration_since(Instant::now());
        match written.recv_timeout(left) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(Failed::Wait(e)),
            Err(_) => return Err(Failed::TimedOut),
        }
    }
    let (out, cut) = out.join().unwrap_or_default();
    let (err, _) = err.join().unwrap_or_default();
    Ok(Done {
        status,
        out,
        cut,
        err,
    })
}
