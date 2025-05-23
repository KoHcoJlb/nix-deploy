use std::{
    collections::VecDeque,
    fs::File,
    io,
    io::{Read, Write},
    os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd},
    process::{Child, Command, ExitStatus, Stdio},
};

use bstr::{BString, ByteVec};
use derive_more::{Display, Error, From};
use eyre::{Context, Report, Result};
use itertools::Itertools;
use polling::{Event, Events, Poller};
use rustix::process::Pid;
use rustix_openpty::openpty;
use serde::de::DeserializeOwned;
use tap::Tap;

use crate::{config::config, line_reader::LineReader};

#[derive(derive_more::Debug, Display, Error)]
#[display("command exited with non-zero: status={status:?}")]
pub struct CommandExit {
    pub status: ExitStatus,
    #[debug(skip)]
    pub stderr: String,
}

#[derive(derive_more::Debug, Display, Error, From)]
pub enum CommandError {
    #[display("exit")]
    Exit(CommandExit),
    #[display("run: {}", cmd.iter().join(" "))]
    Command {
        cmd: Vec<String>,
        source: Box<CommandError>,
    },
    #[display("{msg}")]
    Context {
        msg: String,
        source: Box<CommandError>,
    },
    Io(#[from] io::Error),
    Other(#[from] Report),
}

impl CommandError {
    fn other<T: Into<Report>>(from: T) -> Self {
        Self::Other(from.into())
    }

    fn wrap<E: Into<CommandError>>(cmd: &Command) -> impl FnOnce(E) -> CommandError + use<E> {
        let mut args = vec![cmd.get_program().to_string_lossy().into_owned()];
        args.extend(cmd.get_args().map(|a| a.to_string_lossy().into_owned()));
        move |err| CommandError::Command { cmd: args, source: Box::new(err.into()) }
    }

    fn msg<E: Into<CommandError>>(msg: impl AsRef<str>) -> impl FnOnce(E) -> CommandError {
        move |err| CommandError::Context { msg: msg.as_ref().into(), source: Box::new(err.into()) }
    }
}

/// This reader maps EIO to EOF due to linux behaviour where PTY master returns EIO when all slaves are closed
pub struct PtyReader<R>(pub R);

impl<R: Read> Read for PtyReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buf) {
            Err(err) if err.raw_os_error() == Some(libc::EIO) => Ok(0),
            res => res,
        }
    }
}

impl<R: AsRawFd> AsRawFd for PtyReader<R> {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl<R: AsFd> AsFd for PtyReader<R> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

pub struct TTYChild {
    child: Child,
    pty: LineReader<PtyReader<File>>,
}

impl TTYChild {
    pub fn spawn(mut cmd: Command, separate_stdout: bool) -> Result<Self> {
        let pty = openpty(None, None).context("openpty")?;

        if separate_stdout {
            cmd.stdout(Stdio::piped());
        } else {
            cmd.stdout(pty.user.try_clone()?);
        }

        cmd.stderr(pty.user);
        cmd.stdin(Stdio::null());

        let child = cmd.spawn()?;

        Ok(Self { child, pty: LineReader::new(PtyReader(File::from(pty.controller)), 1024 * 1024) })
    }

    pub fn pid(&self) -> Pid {
        Pid::from_child(&self.child)
    }

    pub fn wait(mut self, on_stderr: impl Fn(String)) -> Result<BString, CommandError> {
        const TTY_KEY: usize = 1;
        const STDOUT_KEY: usize = 2;

        let poller = Poller::new().context("create poller")?;
        unsafe {
            poller.add(&self.pty, Event::readable(TTY_KEY)).context("add tty")?;

            if let Some(stdout) = &self.child.stdout {
                poller.add(stdout, Event::readable(STDOUT_KEY)).context("add stdout")?;
            }
        }

        let mut stdout_buf = vec![];
        let mut stderr_buf = VecDeque::with_capacity(200);

        let mut events = Events::new();
        let mut buf = [0; 64 * 1024];
        'outer: loop {
            events.clear();
            poller.wait(&mut events, None).context("wait for events")?;

            for ev in events.iter() {
                match ev.key {
                    TTY_KEY => {
                        use crate::line_reader::Line;

                        poller.modify(&self.pty, Event::readable(TTY_KEY)).context("re-add tty")?;

                        match self.pty.read_line().context("read line")? {
                            Line::Line(line) => {
                                if stderr_buf.len() == stderr_buf.capacity() - 1 {
                                    stderr_buf.pop_front();
                                }
                                stderr_buf.push_back(line.clone());

                                on_stderr(line);
                            }
                            Line::Incomplete => continue,
                            Line::Eof => break 'outer,
                        }
                    }
                    STDOUT_KEY => {
                        let stdout = self
                            .child
                            .stdout
                            .as_mut()
                            .expect("received stdout event while stdout is empty");

                        poller
                            .modify(&stdout, Event::readable(STDOUT_KEY))
                            .context("re-add stdout")?;

                        let n = stdout.read(&mut buf).context("read stdout")?;
                        stdout_buf.write_all(&buf[..n]).unwrap();
                    }
                    _ => unreachable!(),
                }
            }
        }

        let status = self.child.wait().context("wait for child")?;
        if status.success() {
            Ok(stdout_buf.into())
        } else {
            Err(CommandError::Exit(CommandExit {
                status,
                stderr: stderr_buf.into_iter().collect::<String>(),
            }))
        }
    }
}

pub fn run_command(mut cmd: Command) -> Result<String, CommandError> {
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    cmd.stdin(Stdio::null());

    let child = cmd.spawn().context("spawn")?;

    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(CommandError::Exit(CommandExit {
            status: out.status,
            stderr: out.stderr.into_string_lossy(),
        }));
    }

    String::from_utf8(out.stdout).map_err(CommandError::other)
}

pub fn run_command_tty(cmd: Command) -> Result<BString, CommandError> {
    let wrapper = CommandError::wrap(&cmd);
    TTYChild::spawn(cmd, true)
        .map_err(CommandError::msg("spawn"))
        .and_then(|c| c.wait(|_| ()))
        .map_err(wrapper)
}

pub fn nix_eval<T: DeserializeOwned>(attr: impl AsRef<str>) -> Result<T> {
    let cmd = Command::new("nix").tap_mut(|cmd| {
        cmd.args(["eval", "--json", "--read-only", "--store"])
            .arg(&config().eval_store)
            .arg(format!("path:./flake#{}", attr.as_ref()));
    });

    serde_json::from_slice(&run_command_tty(cmd)?).context("parse nix eval result")
}
