//! A child process with a deadline: `daisy-cli reviews get` and merge's
//! gh and herdr calls ([`cli`]), git calls,
//! agent sessions and the commands of `check run tests`.

use std::ffi::OsStr;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long an agent has to stop its tools before its group is killed.
const GRACE: Duration = Duration::from_secs(2);
const OUT_CAP: usize = 1 << 20;

pub struct Done {
    pub status: ExitStatus,
    /// At most the cap's bytes of stdout.
    pub out: Vec<u8>,
    /// Stdout went past the cap; for [`run_tail`], either stream lost its
    /// start.
    pub cut: bool,
    pub err: Vec<u8>,
    /// [`run_tail`] only: the deadline stopped it, and the streams are what
    /// it printed before.
    pub timed_out: bool,
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
    start(cmd, None, false, deadline, Keep::Head(cap), None)
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
    start(cmd, input, true, deadline, Keep::Head(cap), None)
}

/// Each line of stdout, as it is printed. One that answers false
/// stops the child as the deadline does, and the run ends as if it had
/// exited.
pub type Watch = Box<dyn FnMut(&[u8]) -> bool + Send>;

/// [`run_input`] that hands each stdout line to `watch` and keeps none of
/// stdout. A line past `line_cap` bytes stops the child, with `cut`.
pub fn run_watched(
    cmd: &mut Command,
    input: Option<&[u8]>,
    deadline: Instant,
    watch: Watch,
    line_cap: usize,
) -> Result<Done, Failed> {
    start(
        cmd,
        input,
        true,
        deadline,
        Keep::Head(line_cap),
        Some(watch),
    )
}

/// [`run_input`] with stdin empty that keeps only the last `tail` bytes of
/// stdout and of stderr, so a command that prints without end costs no
/// more memory than that; `cut` says either stream lost its start. At the
/// deadline the child is stopped as [`run_input`] stops it and what it
/// printed is still returned, with `timed_out`. A process that left the
/// group and holds a pipe past the deadline, or past the grace after the
/// stop, ends the wait the same way: what was read so far, also `cut`.
pub fn run_tail(cmd: &mut Command, deadline: Instant, tail: usize) -> Result<Done, Failed> {
    start(cmd, None, true, deadline, Keep::Tail(tail), None)
}

/// The `name` CLI at `cmd` with `args`: its stdout, else its stderr. Run
/// from the temp dir, so gh never reads or changes a checkout (`-R`
/// names the repo). A call past `limit` fails; `late` ends the timeout's
/// message.
pub fn cli(
    name: &str,
    cmd: &OsStr,
    args: &[&str],
    limit: Duration,
    late: &str,
) -> Result<String, String> {
    let call = format!("{name} {}", args[..args.len().min(2)].join(" "));
    let deadline = Instant::now() + limit;
    let mut command = Command::new(cmd);
    command.args(args).current_dir(std::env::temp_dir());
    let out = run(&mut command, deadline, OUT_CAP).map_err(|e| match e {
        Failed::Spawn(e) => format!("{name} did not start: {e}"),
        Failed::TimedOut => format!("{call}: no answer in {}s{late}", limit.as_secs()),
        Failed::Wait(e) => format!("{call}: {e}"),
    })?;
    if out.cut {
        return Err(format!("{call}: answer past {OUT_CAP} bytes"));
    }
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.out).into_owned());
    }
    let err = String::from_utf8_lossy(&out.err);
    Err(format!("{call}: {}", err.trim()))
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
    watch: Option<Watch>,
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
    let halt = Arc::new(AtomicBool::new(false));
    let drain = |pipe: Option<Box<dyn Read + Send>>, keep: Keep, mut watch: Option<Watch>| {
        let (tx, rx) = std::sync::mpsc::channel();
        let kept = Arc::new(Mutex::new((Vec::new(), false)));
        let shared = Arc::clone(&kept);
        let halt = Arc::clone(&halt);
        std::thread::spawn(move || {
            if let Some(mut pipe) = pipe {
                let mut chunk = [0u8; 8192];
                let mut line = Vec::new();
                loop {
                    let n = match pipe.read(&mut chunk) {
                        Ok(0) => {
                            // The last line may end without a newline.
                            if let Some(watch) = watch.as_mut().filter(|_| !line.is_empty()) {
                                watch(&line);
                            }
                            break;
                        }
                        Ok(n) => n,
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    };
                    if let Some(watch) = watch.as_mut() {
                        let mut from = line.len();
                        line.extend_from_slice(&chunk[..n]);
                        while let Some(at) = line[from..].iter().position(|&b| b == b'\n') {
                            let end = from + at;
                            if !halt.load(Ordering::Relaxed) && !watch(&line[..end]) {
                                halt.store(true, Ordering::Relaxed);
                            }
                            line.drain(..=end);
                            from = 0;
                        }
                        if matches!(keep, Keep::Head(cap) if line.len() > cap) {
                            halt.store(true, Ordering::Relaxed);
                            lock(&shared).1 = true;
                            line.clear();
                        }
                        continue;
                    }
                    let (buf, cut) = &mut *lock(&shared);
                    match keep {
                        Keep::Head(cap) => {
                            let kept = n.min(cap - buf.len());
                            buf.extend_from_slice(&chunk[..kept]);
                            *cut |= kept < n;
                        }
                        Keep::Tail(tail) => {
                            buf.extend_from_slice(&chunk[..n]);
                            let over = buf.len().saturating_sub(tail);
                            buf.drain(..over);
                            *cut |= over > 0;
                        }
                    }
                }
            }
            let _ = tx.send(());
        });
        Drain { ended: rx, kept }
    };
    let err_keep = match keep {
        Keep::Head(_) => Keep::Head(usize::MAX),
        tail => tail,
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as _), keep, watch);
    let err = drain(
        child.stderr.take().map(|p| Box::new(p) as _),
        err_keep,
        None,
    );
    let halted = || halt.load(Ordering::Relaxed);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline && !halted() => {
                std::thread::sleep(Duration::from_millis(2))
            }
            Ok(None) => {
                let stopped = if group { stop(&mut child) } else { Ok(()) };
                let _ = child.kill();
                let status = child.wait();
                if let (true, Ok(()), Ok(status)) = (halted(), &stopped, &status) {
                    break *status;
                }
                if let Err(e) = stopped {
                    return Err(Failed::Wait(std::io::Error::other(format!(
                        "timed out, and its process group was not killed: {e}"
                    ))));
                }
                // With its group killed the pipes close; the grace bounds a
                // process that left the group and still holds one, whose
                // stream then counts as cut.
                if let (Keep::Tail(_), Ok(status)) = (keep, status) {
                    let until = Instant::now() + GRACE;
                    let left = || until.saturating_duration_since(Instant::now());
                    let ended = out.ended(left()) & err.ended(left());
                    return Ok(tail(status, out, err, ended, true));
                }
                return Err(Failed::TimedOut);
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
    if input.is_some() && !halted() {
        let left = deadline.saturating_duration_since(Instant::now());
        match written.recv_timeout(left) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(Failed::Wait(e)),
            Err(_) => return Err(Failed::TimedOut),
        }
    }
    // A pipe still held at the deadline, or a reader that panicked: no
    // result for `run`, and `run_tail` keeps what was read.
    let left = || deadline.saturating_duration_since(Instant::now());
    let ended = out.ended(left()) & err.ended(left());
    match keep {
        Keep::Tail(_) => Ok(tail(status, out, err, ended, !ended)),
        Keep::Head(_) if !ended => Err(Failed::TimedOut),
        Keep::Head(_) => {
            let ((out, cut), (err, _)) = (out.take(), err.take());
            Ok(Done {
                status,
                out,
                cut,
                err,
                timed_out: false,
            })
        }
    }
}

/// [`run_tail`]'s result: what both readers kept, cut when either lost
/// its start or had not `ended`.
fn tail(status: ExitStatus, out: Drain, err: Drain, ended: bool, timed_out: bool) -> Done {
    let ((out, out_cut), (err, err_cut)) = (out.take(), err.take());
    Done {
        status,
        out,
        cut: out_cut || err_cut || !ended,
        err,
        timed_out,
    }
}

/// A pipe's reader: what it has kept so far, readable before the pipe
/// ends, and a channel it signals at the end, so the wait for it has the
/// deadline.
struct Drain {
    ended: Receiver<()>,
    kept: Arc<Mutex<(Vec<u8>, bool)>>,
}

impl Drain {
    fn ended(&self, wait: Duration) -> bool {
        self.ended.recv_timeout(wait).is_ok()
    }

    /// The bytes kept and whether some were dropped.
    fn take(self) -> (Vec<u8>, bool) {
        std::mem::take(&mut *lock(&self.kept))
    }
}

/// A reader that panicked under the lock still leaves what it kept.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
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
