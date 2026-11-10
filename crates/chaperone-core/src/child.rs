//! A child process with a deadline: `daisy-cli reviews get`, the UI
//! backlog's daisy reads, the UI writer's gh and herdr calls, the git
//! calls of `overlap`, `impact` and the UI's diff, agent sessions and the
//! commands of `check run tests`.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// How long an agent has to stop its tools before its group is killed.
const GRACE: Duration = Duration::from_secs(2);

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
/// A process the child left behind that still holds a pipe at the
/// deadline makes the run [`Failed::TimedOut`], even after a clean exit.
pub fn run(cmd: &mut Command, deadline: Instant, cap: usize) -> Result<Done, Failed> {
    start(cmd, None, false, deadline, Keep::Head(cap))
}

/// [`run`] for an agent session, with `input` written to the child's
/// stdin, which is then closed: a prompt too long for an argument. A
/// child that exits before it has read all of it is a [`Failed::Wait`],
/// whatever it printed. The child leads a process group of its own. At the
/// deadline it gets SIGTERM and 2 s to stop, then its group gets
/// SIGKILL. That reaches what the agent started in its own group; a tool
/// it put in a group or session of its own goes only if the agent stops
/// it on SIGTERM. Once the child has exited its group is killed too, so
/// a process it left in the group neither holds a pipe until the deadline
/// nor outlives the run. The cost: a Ctrl-C at the terminal no longer
/// reaches the session, and a chaperone that dies leaves the session
/// running.
pub fn run_input(
    cmd: &mut Command,
    input: Option<&[u8]>,
    deadline: Instant,
    cap: usize,
) -> Result<Done, Failed> {
    start(cmd, input, true, deadline, Keep::Head(cap))
}

/// [`run_input`] with stdin empty that keeps only the last `tail` bytes of
/// stdout and of stderr, so a command that prints without end costs no
/// more memory than that; `cut` says stdout lost its start.
pub fn run_tail(cmd: &mut Command, deadline: Instant, tail: usize) -> Result<Done, Failed> {
    start(cmd, None, true, deadline, Keep::Tail(tail))
}

/// What a pipe's reader keeps: the first bytes (stdout's cap, stderr whole)
/// or the last bytes of both.
#[derive(Clone, Copy)]
enum Keep {
    Head(usize),
    Tail(usize),
}

fn start(
    cmd: &mut Command,
    input: Option<&[u8]>,
    group: bool,
    deadline: Instant,
    keep: Keep,
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
    // Over a channel, so the wait for the end of a pipe has the deadline.
    let drain = |pipe: Option<Box<dyn Read + Send>>, keep: Keep| {
        let (tx, rx) = std::sync::mpsc::channel();
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
                    match keep {
                        Keep::Head(cap) => {
                            let kept = n.min(cap - buf.len());
                            buf.extend_from_slice(&chunk[..kept]);
                            cut |= kept < n;
                        }
                        Keep::Tail(tail) => {
                            buf.extend_from_slice(&chunk[..n]);
                            let over = buf.len().saturating_sub(tail);
                            buf.drain(..over);
                            cut |= over > 0;
                        }
                    }
                }
            }
            let _ = tx.send((buf, cut));
        });
        rx
    };
    let err_keep = match keep {
        Keep::Head(_) => Keep::Head(usize::MAX),
        tail => tail,
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as _), keep);
    let err = drain(child.stderr.take().map(|p| Box::new(p) as _), err_keep);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            Ok(None) => {
                let stopped = if group { stop(&mut child) } else { Ok(()) };
                let _ = child.kill();
                let _ = child.wait();
                return Err(match stopped {
                    Ok(()) => Failed::TimedOut,
                    Err(e) => Failed::Wait(std::io::Error::other(format!(
                        "timed out, and its process group was not killed: {e}"
                    ))),
                });
            }
            Err(e) => return Err(Failed::Wait(e)),
        }
    };
    // What the child left in its group would hold the pipes to the
    // deadline. While one is alive the group's id cannot be reused; with
    // none left, the kill finds nothing.
    if group {
        let _ = signal("-KILL", format!("-{}", child.id()));
    }
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
    // A reader only stops early by panicking, which is no result either.
    let left = || deadline.saturating_duration_since(Instant::now());
    let (out, cut) = out.recv_timeout(left()).map_err(|_| Failed::TimedOut)?;
    let (err, _) = err.recv_timeout(left()).map_err(|_| Failed::TimedOut)?;
    Ok(Done {
        status,
        out,
        cut,
        err,
    })
}

/// SIGTERM to the agent, so it can stop the tools it started, then
/// SIGKILL to its group after at most [`GRACE`]. std has no killpg and
/// `unsafe` is forbidden, so the system's kill sends both.
fn stop(child: &mut Child) -> std::io::Result<()> {
    let pid = child.id();
    let _ = signal("-TERM", pid.to_string());
    let until = Instant::now() + GRACE;
    let mut alive = true;
    while alive && Instant::now() < until {
        alive = child.try_wait()?.is_none();
        std::thread::sleep(Duration::from_millis(10));
    }
    // While the agent is unreaped its group exists, so a failed kill is a
    // real failure. Once reaped, the group may be empty and kill finds
    // nothing; its pid names a new group only after the pids wrap around.
    match signal("-KILL", format!("-{pid}"))? {
        status if alive && !status.success() => Err(std::io::Error::other(format!(
            "/bin/kill -KILL -- -{pid}: {status}"
        ))),
        _ => Ok(()),
    }
}

/// `kill <signal> -- <target>`: a pid, or `-<pgid>` for a group.
fn signal(signal: &str, target: String) -> std::io::Result<ExitStatus> {
    Command::new("/bin/kill")
        .args([signal, "--", &target])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
}
