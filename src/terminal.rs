use std::{
    fmt::Write as FmtWrite,
    fs::File,
    io,
    io::{Stdout, Write, stdout},
    ops::Deref,
    os::fd::AsFd,
    process::Command,
    sync::OnceLock,
    time::{Duration, Instant},
};

use ansi_to_tui::IntoText;
use crossterm::{
    event,
    event::{Event, KeyCode, KeyEvent, KeyModifiers},
};
use derive_more::{Display, Error};
use eyre::{Report, Result, ensure};
use parking_lot::{Mutex, MutexGuard};
use ratatui::{
    Frame, TerminalOptions, Viewport,
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Position},
    prelude::{Line, Stylize},
    widgets::{Paragraph, Widget, Wrap},
};
use rustix::termios::{OptionalActions::Now, OutputModes, Termios, tcgetattr, tcsetattr};
use tap::{Pipe, Tap};
use tracing::error;
use tracing_subscriber::fmt::{format::Writer, time::FormatTime};

use crate::{
    command::{CommandExit, TTYChild},
    util::downcast_ref,
};

type RawTerminal<W = File> = ratatui::Terminal<CrosstermBackend<W>>;

fn clear_terminal<B: Backend>(terminal: &mut ratatui::Terminal<B>) -> Result<(), B::Error> {
    let origin = terminal.get_frame().area().as_position();
    terminal.clear()?;
    terminal.set_cursor_position(origin)
}

fn insert_output<B: Backend>(terminal: &mut ratatui::Terminal<B>, bytes: &[u8]) -> io::Result<()>
where
    B::Error: Send + Sync + 'static,
{
    terminal.autoresize().map_err(io::Error::other)?;

    let text = bytes.into_text().map_err(io::Error::other)?;
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    let height = paragraph
        .line_count(terminal.get_frame().area().width)
        .try_into()
        .map_err(io::Error::other)?;
    terminal
        .insert_before(height, |buffer| paragraph.render(buffer.area, buffer))
        .map_err(io::Error::other)?;

    // Keep cursor tracking anchored to the moved viewport for subsequent resizes.
    let origin = terminal.get_frame().area().as_position();
    terminal.set_cursor_position(origin).map_err(io::Error::other)
}

pub fn create_terminal(height: u16) -> RawTerminal<Stdout> {
    let backend = CrosstermBackend::new(stdout());
    RawTerminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(height) })
        .unwrap()
}

pub fn print_error(err: Report) {
    if let Some(CommandExit { stderr, .. }) = downcast_ref(&err) {
        error!(?err, stderr = %format!("\n{stderr}"));
    } else {
        error!(?err);
    }
}

#[derive(Debug, Display, Error)]
#[display("title: {text}")]
pub struct ErrorStyle {
    text: Line<'static>,
    top: bool,
    #[error(source)]
    inner: Report,
}

impl ErrorStyle {
    pub fn map<E: Into<Report>>(
        text: impl Into<Line<'static>>, top: bool,
    ) -> impl FnOnce(E) -> Report {
        move |err| Self { text: text.into(), top, inner: err.into() }.into()
    }

    pub fn system<E: Into<Report>>(system_name: &str) -> impl FnOnce(E) -> Report {
        ErrorStyle::map(
            Line::from(system_name.to_owned()).alignment(Alignment::Center).bold(),
            true,
        )
    }

    pub fn action<E: Into<Report>>(action: &str) -> impl FnOnce(E) -> Report {
        ErrorStyle::map(Line::from(action.to_owned()).alignment(Alignment::Left).dark_gray(), true)
    }

    pub fn system_and_action<'a, E: Into<Report>>(
        system_name: &'a str, action: &'a str,
    ) -> impl FnOnce(E) -> Report + use<'a, E> {
        move |err| err.pipe(ErrorStyle::system(system_name)).pipe(ErrorStyle::action(action))
    }
}

pub fn handle_ctrlc(dur: Duration) -> bool {
    if event::poll(dur).unwrap() {
        let ev = crossterm::event::read().unwrap();
        if let Event::Key(KeyEvent { code, modifiers, .. }) = ev
            && modifiers == KeyModifiers::CONTROL
            && code == KeyCode::Char('c')
        {
            return true;
        }
    }

    false
}

struct Inner {
    writer: File,
    terminal: Option<RawTerminal>,
    height: u16,

    orig_mode: Termios,
    raw_mode: Termios,
    write_mode: Termios,
}

impl Inner {
    fn get_terminal(&mut self) -> Result<&mut RawTerminal> {
        ensure!(self.height > 0, "zero height terminal");

        match &mut self.terminal {
            Some(terminal) => Ok(terminal),
            terminal => Ok(terminal.insert(RawTerminal::with_options(
                CrosstermBackend::new(self.writer.try_clone()?),
                TerminalOptions { viewport: Viewport::Inline(self.height) },
            )?)),
        }
    }
}

pub struct Terminal {
    inner: Mutex<Inner>,
}

impl Terminal {
    fn init() -> Result<Self> {
        let writer = File::from(stdout().as_fd().try_clone_to_owned()?);

        let orig_mode = tcgetattr(&writer)?;
        let raw_mode = orig_mode.clone().tap_mut(Termios::make_raw);
        let write_mode =
            raw_mode.clone().tap_mut(|t| t.output_modes |= OutputModes::OPOST | OutputModes::ONLCR);

        tcsetattr(&writer, Now, &raw_mode)?;

        Ok(Self {
            inner: Inner { orig_mode, raw_mode, write_mode, writer, height: 0, terminal: None }
                .into(),
        })
    }

    pub fn writer(&self) -> impl Write {
        TerminalWriter { inner: self.inner.lock(), buffer: Vec::new() }
    }

    pub fn run_command(&self, cmd: Command) -> Result<()> {
        TTYChild::spawn(cmd, false)?.wait(|line| {
            self.writer().write_all(line.as_bytes()).unwrap();
        })?;

        Ok(())
    }

    pub fn resize(&self, height: u16) -> Result<()> {
        let mut inner = self.inner.lock();
        if inner.height == height {
            return Ok(());
        }

        if let Some(mut terminal) = inner.terminal.take() {
            clear_terminal(&mut terminal)?;
        }
        inner.height = height;

        Ok(())
    }

    pub fn draw<F>(&self, render_callback: F) -> Result<()>
    where
        F: FnOnce(&mut Frame),
    {
        self.inner.lock().get_terminal()?.draw(render_callback)?;
        Ok(())
    }

    fn drop(&self) {
        let mut inner = self.inner.lock();
        if let Some(mut terminal) = inner.terminal.take() {
            let area = terminal.get_frame().area();
            let _ = terminal.set_cursor_position(Position::new(area.right(), area.bottom() - 1));
        }
        let _ = tcsetattr(&inner.writer, Now, &inner.orig_mode);
    }
}

struct TerminalWriter<'a> {
    inner: MutexGuard<'a, Inner>,
    buffer: Vec<u8>,
}

impl Write for TerminalWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.buffer.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.buffer.is_empty() {
            return Ok(());
        }

        // Scrolling regions need at least two rows; retain normal output in very short terminals.
        if let Some(terminal) = &mut self.inner.terminal
            && terminal.get_frame().area().height.saturating_add(1) >= terminal.size()?.height
        {
            clear_terminal(terminal)?;
            self.inner.terminal.take();
        }

        if let Some(terminal) = &mut self.inner.terminal {
            insert_output(terminal, &self.buffer)?;
        } else {
            tcsetattr(&self.inner.writer, Now, &self.inner.write_mode)?;
            let result = self.inner.writer.write_all(&self.buffer);
            tcsetattr(&self.inner.writer, Now, &self.inner.raw_mode)?;
            result?;
        }
        self.buffer.clear();

        Ok(())
    }
}

impl Drop for TerminalWriter<'_> {
    fn drop(&mut self) {
        self.flush().unwrap();
    }
}

pub struct TerminalRef(OnceLock<Terminal>);

impl TerminalRef {
    pub fn init(&self) -> Result<impl Drop> {
        assert!(self.0.get().is_none(), "terminal already initialized");
        let _ = self.0.set(Terminal::init()?);

        Ok(scopeguard::guard((), |_| {
            self.drop();
        }))
    }
}

impl Deref for TerminalRef {
    type Target = Terminal;

    fn deref(&self) -> &Self::Target {
        self.0.get().expect("terminal not initialized")
    }
}

pub static TERMINAL: TerminalRef = TerminalRef(OnceLock::new());

pub struct Uptime(Instant);

impl Default for Uptime {
    fn default() -> Self {
        Self(Instant::now())
    }
}

impl FormatTime for Uptime {
    fn format_time(&self, w: &mut Writer<'_>) -> std::fmt::Result {
        let mut buf = String::new();

        let elapsed = self.0.elapsed();
        if let minutes = elapsed.as_secs() / 60
            && minutes > 0
        {
            write!(buf, "{minutes}m")?;
        }
        write!(buf, "{}.{}s", elapsed.as_secs() % 60, elapsed.subsec_millis())?;

        write!(w, "{:>10}", buf)
    }
}

#[cfg(test)]
mod tests {
    use ratatui::{
        Terminal, TerminalOptions, Viewport,
        backend::{Backend, TestBackend},
        buffer::Buffer,
        layout::{Position, Rect},
        style::{Color, Style},
        text::Text,
    };

    use super::{clear_terminal, insert_output};

    #[test]
    fn output_moves_status_rows_without_redrawing_them() {
        let mut terminal = Terminal::with_options(
            TestBackend::new(16, 8),
            TerminalOptions { viewport: Viewport::Inline(2) },
        )
        .unwrap();
        let statuses = Text::from("alpha\nbeta");
        terminal.draw(|frame| frame.render_widget(&statuses, frame.area())).unwrap();

        insert_output(&mut terminal, b"\x1b[32m0s INFO built\x1b[0m\n  detail wraps over\n\n")
            .unwrap();

        assert_eq!(terminal.get_frame().area().y, 4);
        let mut expected = Buffer::with_lines([
            "0s INFO built   ",
            "  detail wraps  ",
            "over            ",
            "                ",
            "alpha           ",
            "beta            ",
            "                ",
            "                ",
        ]);
        expected.set_style(Rect::new(0, 0, 13, 1), Style::new().fg(Color::Green));
        terminal.backend().assert_buffer(&expected);

        insert_output(&mut terminal, b"first\nsecond\nthird\n").unwrap();

        assert_eq!(terminal.get_frame().area().y, 6);
        let expected = Buffer::with_lines([
            "  detail wraps  ",
            "over            ",
            "                ",
            "first           ",
            "second          ",
            "third           ",
            "alpha           ",
            "beta            ",
        ]);
        terminal.backend().assert_buffer(&expected);

        terminal.draw(|frame| frame.render_widget(&statuses, frame.area())).unwrap();

        terminal.backend().assert_buffer(&expected);

        terminal.backend_mut().resize(16, 10);
        terminal.draw(|frame| frame.render_widget(&statuses, frame.area())).unwrap();

        assert_eq!(terminal.get_frame().area().y, 6);
    }

    #[test]
    fn growing_status_viewport_preserves_logs_and_origin() {
        let mut backend = TestBackend::with_lines([
            "first log ",
            "second log",
            "          ",
            "          ",
            "          ",
            "          ",
        ]);
        backend.set_cursor_position((0, 2)).unwrap();
        let mut terminal =
            Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(2) })
                .unwrap();
        terminal
            .draw(|frame| frame.render_widget(Text::from("alpha\nbeta"), frame.area()))
            .unwrap();

        clear_terminal(&mut terminal).unwrap();
        let mut terminal = Terminal::with_options(
            terminal.backend().clone(),
            TerminalOptions { viewport: Viewport::Inline(3) },
        )
        .unwrap();
        terminal
            .draw(|frame| frame.render_widget(Text::from("alpha\nbeta\ngamma"), frame.area()))
            .unwrap();

        assert_eq!(terminal.get_frame().area().y, 2);
        terminal.backend().assert_buffer_lines([
            "first log ",
            "second log",
            "alpha     ",
            "beta      ",
            "gamma     ",
            "          ",
        ]);
    }

    #[test]
    fn clear_terminal_resets_cursor_to_viewport_origin() {
        let backend = TestBackend::new(80, 10);
        let mut terminal =
            Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(1) })
                .unwrap();
        let origin = terminal.get_frame().area().as_position();
        terminal.set_cursor_position(Position::new(70, origin.y)).unwrap();

        clear_terminal(&mut terminal).unwrap();

        assert_eq!(terminal.get_cursor_position().unwrap(), origin);
    }
}
