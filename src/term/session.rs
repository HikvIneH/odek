//! One shell: a pty, the child process, a reader thread that feeds `Term`, and
//! a writer thread so a big paste never blocks the UI.

use std::ffi::{CStr, CString, OsStr, OsString};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::vt::Term;

/// Wait this long for the end of a synchronized update before drawing anyway.
const SYNC_TIMEOUT: Duration = Duration::from_millis(150);

/// Environment variables from whatever launched us that would confuse
/// programs into thinking they run in another terminal.
const DROP_ENV: &[&str] = &[
    "TERM",
    "COLORTERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    "CLAUDECODE",
];
const DROP_ENV_PREFIX: &[&str] = &["ITERM_", "WARP_", "LC_TERMINAL", "CLAUDE_CODE_"];

pub struct Spawn<'a> {
    pub cwd: &'a Path,
    /// Run this through the login shell instead of an interactive shell.
    pub command: Option<&'a str>,
    pub cols: u16,
    pub rows: u16,
    pub cell_px: (u16, u16),
    /// Set on top of odek's own environment (a secluded profile's HOME).
    pub env: &'a [(OsString, OsString)],
}

pub struct Session {
    pub term: Arc<Mutex<Term>>,
    master: OwnedFd,
    pid: libc::pid_t,
    tx: Sender<Vec<u8>>,
    /// A redraw has been requested and not yet handled.
    pub wake_pending: Arc<AtomicBool>,
    pub exited: Arc<Mutex<Option<i32>>>,
}

impl Session {
    /// `wake` runs on the reader thread whenever the screen changed; it
    /// should hop to the main thread and redraw.
    pub fn spawn(opts: &Spawn, wake: Arc<dyn Fn() + Send + Sync>) -> io::Result<Session> {
        let (master, pid) = fork_shell(opts)?;
        let term = Arc::new(Mutex::new(Term::new(opts.cols as usize, opts.rows as usize)));
        let wake_pending = Arc::new(AtomicBool::new(false));
        let exited = Arc::new(Mutex::new(None));

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let mut writer = File::from(master.try_clone()?);
        std::thread::Builder::new()
            .name("pty-writer".into())
            .stack_size(64 << 10)
            .spawn(move || {
                for chunk in rx {
                    if writer.write_all(&chunk).is_err() {
                        break;
                    }
                }
            })?;

        let reader = File::from(master.try_clone()?);
        let ctx = Reader {
            file: reader,
            term: term.clone(),
            tx: tx.clone(),
            wake,
            wake_pending: wake_pending.clone(),
            exited: exited.clone(),
            pid,
        };
        std::thread::Builder::new()
            .name("pty-reader".into())
            .stack_size(256 << 10)
            .spawn(move || ctx.run())?;

        Ok(Session {
            term,
            master,
            pid,
            tx,
            wake_pending,
            exited,
        })
    }

    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        let _ = self.tx.send(bytes.into());
    }

    pub fn resize(&self, cols: u16, rows: u16, cell_px: (u16, u16)) {
        self.term.lock().unwrap().resize(cols as usize, rows as usize);
        let ws = winsize(cols, rows, cell_px);
        unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &ws) };
    }

    /// The program in the foreground (e.g. "claude", "vim"), or the shell.
    fn foreground_pid(&self) -> libc::pid_t {
        let pgrp = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        if pgrp > 0 { pgrp } else { self.pid }
    }

    /// The foreground program's arguments (`["claude", "--model", "fable"]`),
    /// or None while the shell itself is in the foreground. Read on demand
    /// (saving a profile), never kept.
    pub fn foreground_args(&self) -> Option<Vec<String>> {
        let pid = self.foreground_pid();
        if pid == self.pid {
            return None;
        }
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size: libc::size_t = 0;
        let null = std::ptr::null_mut();
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, null, &mut size, null, 0) } != 0 {
            return None;
        }
        let mut buf = vec![0u8; size];
        if unsafe { libc::sysctl(mib.as_mut_ptr(), 3, buf.as_mut_ptr().cast(), &mut size, null, 0) } != 0 {
            return None;
        }
        buf.truncate(size);
        parse_procargs(&buf)
    }

    pub fn foreground_name(&self) -> Option<String> {
        let pid = self.foreground_pid();
        let mut path = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let n = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
        if n > 0 && is_claude_build(Path::new(OsStr::from_bytes(&path[..n as usize]))) {
            return Some("claude".into());
        }
        let mut buf = [0u8; 256];
        let n = unsafe { libc::proc_name(pid, buf.as_mut_ptr().cast(), buf.len() as u32) };
        (n > 0).then(|| String::from_utf8_lossy(&buf[..n as usize]).into_owned())
    }

    /// Working directory of the foreground process; needs no shell hooks.
    pub fn cwd(&self) -> Option<PathBuf> {
        for pid in [self.foreground_pid(), self.pid] {
            let mut info: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
            let size = size_of::<libc::proc_vnodepathinfo>() as libc::c_int;
            let n = unsafe {
                libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, (&raw mut info).cast(), size)
            };
            if n == size {
                let raw = unsafe { CStr::from_ptr(info.pvi_cdir.vip_path.as_ptr().cast()) };
                return Some(PathBuf::from(OsStr::from_bytes(raw.to_bytes())));
            }
        }
        None
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Hang up the shell; it passes SIGHUP on to its jobs. The reader
        // thread then sees EOF and reaps the child.
        unsafe { libc::kill(self.pid, libc::SIGHUP) };
    }
}

struct Reader {
    file: File,
    term: Arc<Mutex<Term>>,
    tx: Sender<Vec<u8>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    wake_pending: Arc<AtomicBool>,
    exited: Arc<Mutex<Option<i32>>>,
    pid: libc::pid_t,
}

impl Reader {
    fn run(mut self) {
        let mut parser = vte::Parser::new();
        let mut buf = vec![0u8; 64 << 10];
        let mut sync_since: Option<Instant> = None;
        loop {
            // Inside a synchronized update, don't wait forever for its end.
            if let Some(since) = sync_since {
                let left = SYNC_TIMEOUT.saturating_sub(since.elapsed());
                let mut pfd = libc::pollfd {
                    fd: self.file.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut pfd, 1, left.as_millis() as libc::c_int) };
                if ready == 0 {
                    sync_since = None;
                    self.wake();
                    continue;
                }
            }
            let n = match self.file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            let (reply, sync) = {
                let mut term = self.term.lock().unwrap();
                parser.advance(&mut *term, &buf[..n]);
                (std::mem::take(&mut term.reply), term.modes.sync)
            };
            if !reply.is_empty() {
                let _ = self.tx.send(reply);
            }
            if sync {
                let since = *sync_since.get_or_insert_with(Instant::now);
                if since.elapsed() < SYNC_TIMEOUT {
                    continue;
                }
            }
            sync_since = None;
            self.wake();
        }
        let mut status = 0;
        unsafe { libc::waitpid(self.pid, &mut status, 0) };
        let code = if libc::WIFEXITED(status) {
            libc::WEXITSTATUS(status)
        } else {
            128 + libc::WTERMSIG(status)
        };
        *self.exited.lock().unwrap() = Some(code);
        self.wake_pending.store(false, Ordering::SeqCst);
        self.wake();
    }

    fn wake(&self) {
        if !self.wake_pending.swap(true, Ordering::SeqCst) {
            (self.wake)();
        }
    }
}

/// A process's arguments from a KERN_PROCARGS2 buffer: argc (int), the
/// executable's path, NUL padding, then argc NUL-terminated arguments.
fn parse_procargs(buf: &[u8]) -> Option<Vec<String>> {
    let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
    let argc = usize::try_from(argc).ok().filter(|&n| n > 0)?;
    let rest = &buf[4..];
    let rest = &rest[rest.iter().position(|&b| b == 0)?..];
    let rest = &rest[rest.iter().position(|&b| b != 0)?..];
    let args: Vec<String> = rest
        .split(|&b| b == 0)
        .take(argc)
        .map(|a| String::from_utf8_lossy(a).into_owned())
        .collect();
    (args.len() == argc).then_some(args)
}

/// Claude Code's native install runs `~/.local/share/claude/versions/2.1.314`,
/// so its process is named after the version, not "claude".
fn is_claude_build(exe: &Path) -> bool {
    let mut up = exe.ancestors().skip(1).filter_map(Path::file_name);
    up.next() == Some(OsStr::new("versions")) && up.next() == Some(OsStr::new("claude"))
}

fn winsize(cols: u16, rows: u16, cell_px: (u16, u16)) -> libc::winsize {
    libc::winsize {
        ws_row: rows,
        ws_col: cols,
        ws_xpixel: cols.saturating_mul(cell_px.0),
        ws_ypixel: rows.saturating_mul(cell_px.1),
    }
}

fn login_shell() -> CString {
    if let Some(s) = std::env::var_os("SHELL").filter(|s| !s.is_empty()) {
        return CString::new(s.as_bytes()).unwrap_or_else(|_| c"/bin/zsh".into());
    }
    let pw = unsafe { libc::getpwuid(libc::getuid()) };
    if !pw.is_null() {
        let shell = unsafe { CStr::from_ptr((*pw).pw_shell) };
        if !shell.is_empty() {
            return shell.into();
        }
    }
    c"/bin/zsh".into()
}

fn child_env(extra: &[(OsString, OsString)]) -> Vec<CString> {
    let mut env: Vec<CString> = std::env::vars_os()
        .filter(|(k, _)| {
            let k = k.to_string_lossy();
            !DROP_ENV.contains(&k.as_ref()) && !DROP_ENV_PREFIX.iter().any(|p| k.starts_with(p))
        })
        .filter(|(k, _)| !extra.iter().any(|(e, _)| e == k))
        .chain(extra.iter().cloned())
        .filter_map(|(k, v)| {
            let mut kv = k.as_bytes().to_vec();
            kv.push(b'=');
            kv.extend_from_slice(v.as_bytes());
            CString::new(kv).ok()
        })
        .collect();
    let version = format!("TERM_PROGRAM_VERSION={}", env!("CARGO_PKG_VERSION"));
    for kv in [
        "TERM=xterm-256color",
        "COLORTERM=truecolor",
        "TERM_PROGRAM=Odek",
        &version,
    ] {
        env.push(CString::new(kv).unwrap());
    }
    // powerlevel10k then marks its prompt (OSC 133), so a resize keeps the
    // prompt intact instead of leaving copies of it behind.
    if std::env::var_os("POWERLEVEL9K_TERM_SHELL_INTEGRATION").is_none() {
        env.push(c"POWERLEVEL9K_TERM_SHELL_INTEGRATION=true".into());
    }
    // Apps opened from Finder get no locale, and zsh then mangles UTF-8.
    if std::env::var_os("LANG").is_none() && std::env::var_os("LC_ALL").is_none() {
        env.push(c"LANG=en_US.UTF-8".into());
    }
    env
}

fn fork_shell(opts: &Spawn) -> io::Result<(OwnedFd, libc::pid_t)> {
    // Everything the child needs is built before fork: after it, only
    // async-signal-safe calls are allowed.
    let shell = login_shell();
    let name = shell
        .to_bytes()
        .rsplit(|&b| b == b'/')
        .next()
        .unwrap_or(b"zsh")
        .to_vec();
    let argv: Vec<CString> = match opts.command {
        Some(cmd) => vec![shell.clone(), c"-l".into(), c"-c".into(), CString::new(cmd)?],
        None => vec![CString::new([b"-".as_slice(), &name].concat())?],
    };
    let mut argv_ptrs: Vec<*const libc::c_char> = argv.iter().map(|s| s.as_ptr()).collect();
    argv_ptrs.push(std::ptr::null());
    let env = child_env(opts.env);
    let mut env_ptrs: Vec<*const libc::c_char> = env.iter().map(|s| s.as_ptr()).collect();
    env_ptrs.push(std::ptr::null());
    let cwd = CString::new(opts.cwd.as_os_str().as_bytes())?;
    let home = CString::new(std::env::var_os("HOME").unwrap_or_default().as_bytes())?;
    let mut ws = winsize(opts.cols, opts.rows, opts.cell_px);
    let mut empty: libc::sigset_t = 0;
    unsafe { libc::sigemptyset(&mut empty) };

    let mut master: libc::c_int = -1;
    let pid = unsafe { libc::forkpty(&mut master, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) };
    if pid < 0 {
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        unsafe {
            if libc::chdir(cwd.as_ptr()) != 0 {
                libc::chdir(home.as_ptr());
            }
            // Rust ignores SIGPIPE; ignored signals survive exec.
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            libc::sigprocmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut());
            libc::execve(shell.as_ptr(), argv_ptrs.as_ptr(), env_ptrs.as_ptr());
            libc::_exit(127);
        }
    }
    unsafe { libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC) };
    Ok((unsafe { OwnedFd::from_raw_fd(master) }, pid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spots_claude_native_builds() {
        assert!(is_claude_build(Path::new(
            "/Users/a/.local/share/claude/versions/2.1.314"
        )));
        assert!(!is_claude_build(Path::new("/opt/homebrew/bin/claude")));
        assert!(!is_claude_build(Path::new("/usr/local/lib/node/versions/22.1.0")));
        assert!(!is_claude_build(Path::new("/versions")));
    }

    #[test]
    fn parses_process_arguments() {
        let mut buf = 3i32.to_ne_bytes().to_vec();
        buf.extend_from_slice(b"/usr/local/bin/claude\0\0\0\0claude\0--model\0fable\0HOME=/x\0");
        assert_eq!(parse_procargs(&buf).unwrap(), ["claude", "--model", "fable"]);
        assert_eq!(parse_procargs(&buf[..4]), None);
        assert_eq!(parse_procargs(&0i32.to_ne_bytes()), None);
        let mut short = 5i32.to_ne_bytes().to_vec();
        short.extend_from_slice(b"/bin/x\0x\0");
        assert_eq!(parse_procargs(&short), None);
    }

    #[test]
    fn runs_a_command_in_a_pty() {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            let _ = tx.lock().unwrap().send(());
        });
        let env = [(OsString::from("ODEK_TEST_HOME"), OsString::from("secluded"))];
        let opts = Spawn {
            cwd: Path::new("/tmp"),
            command: Some("printf 'cols=%s\\n' $(tput cols); pwd; echo $TERM_PROGRAM $ODEK_TEST_HOME"),
            cols: 77,
            rows: 10,
            cell_px: (8, 16),
            env: &env,
        };
        let s = Session::spawn(&opts, wake).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while s.exited.lock().unwrap().is_none() && Instant::now() < deadline {
            let _ = rx.recv_timeout(Duration::from_millis(100));
            s.wake_pending.store(false, Ordering::SeqCst);
        }
        let text = s.term.lock().unwrap().screen_text();
        assert!(text.contains("cols=77"), "{text}");
        assert!(text.contains("/private/tmp") || text.contains("\n/tmp"), "{text}");
        assert!(text.contains("Odek secluded"), "{text}");
        assert_eq!(*s.exited.lock().unwrap(), Some(0));
    }
}
