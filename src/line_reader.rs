use std::{
    io::Read,
    os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd},
};

use circular::Buffer;
use eyre::{Result, bail};
use winnow::{
    ModalResult, Parser, Partial,
    combinator::{alt, repeat_till},
    error::ErrMode,
    stream::{Offset, Stream, StreamIsPartial},
    token::{any, one_of},
};

fn line_ending(input: &mut Partial<&[u8]>) -> ModalResult<()> {
    if !input.is_partial() && input.eof_offset() == 0 {
        return Ok(());
    }

    alt(("\r\n".void(), one_of(['\r', '\n']).void())).parse_next(input)
}

pub fn line<'s>(input: &mut Partial<&'s [u8]>) -> ModalResult<&'s [u8]> {
    repeat_till(0.., any, line_ending).map(|((), _)| ()).take().parse_next(input)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    Line(String),
    Incomplete,
    Eof,
}

pub struct LineReader<R> {
    inner: R,
    buf: Buffer,
}

impl<R: AsRawFd> AsRawFd for LineReader<R> {
    fn as_raw_fd(&self) -> RawFd {
        self.inner.as_raw_fd()
    }
}

impl<R: AsFd> AsFd for LineReader<R> {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

impl<R: Read> LineReader<R> {
    pub fn new(inner: R, capacity: usize) -> Self {
        Self { inner, buf: Buffer::with_capacity(capacity) }
    }

    pub fn read_line(&mut self) -> Result<Line> {
        let n = self.inner.read(self.buf.space())?;
        if n == 0 && self.buf.available_data() == 0 {
            return Ok(Line::Eof);
        }
        self.buf.fill(n);

        let mut partial = Partial::new(self.buf.data());
        if n == 0 {
            let _ = partial.complete();
        }
        let start = partial.checkpoint();

        let line = match line.parse_next(&mut partial) {
            Ok(line) => line,
            Err(ErrMode::Incomplete(_)) => return Ok(Line::Incomplete),
            Err(err) => bail!("parse line: {err:?}"),
        };
        let line = String::from_utf8_lossy(line).into_owned();

        self.buf.consume(partial.offset_from(&start));

        Ok(Line::Line(line))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use eyre::Result;

    use crate::line_reader::{Line, LineReader};

    #[test]
    fn test_line_reader() -> Result<()> {
        let mut reader = LineReader::new(Cursor::new("test"), 1024);
        let res = reader.read_line()?;
        assert_eq!(res, Line::Line("test".to_owned()));
        Ok(())
    }
}
