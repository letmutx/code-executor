use hyper::Body;
use bytes::BytesMut;
use bytes::{BigEndian, ByteOrder};

use docker::DockerError;

use futures::{Async, Poll, Stream};

pub struct Logs {
    body: Body,
    finished: bool,
    state: State,
    buf: BytesMut,
}

enum State {
    Head,
    Body(Header),
}

#[derive(Debug, PartialEq)]
enum LogType {
    Stdout,
    Stdin,
    Stderr,
}

#[derive(Debug)]
struct Header {
    pub log_type: LogType,
    pub size: u32,
}

impl Header {
    fn new(bytes: &[u8]) -> Header {
        let log_type = match bytes[0] {
            0u8 => LogType::Stdin,
            1u8 => LogType::Stdout,
            2u8 => LogType::Stderr,
            _ => unreachable!(),
        };
        let size = BigEndian::read_u32(&bytes[4..]);
        Header {
            log_type: log_type,
            size: size,
        }
    }
}

impl Logs {
    pub fn new(body: Body) -> Self {
        Logs {
            body: body,
            finished: false,
            state: State::Head,
            buf: BytesMut::with_capacity(64),
        }
    }

    fn can_read_head(&self) -> bool {
        let can = match self.state {
            State::Head if self.buf.len() >= 8 => true,
            _ => false,
        };

        trace!("can_read_head: {}", can);
        can
    }

    fn read_head(&mut self) -> Option<Header> {
        match self.state {
            State::Head => {
                let buf = self.buf.split_to(8);
                Some(Header::new(&buf))
            }
            _ => None,
        }
    }

    fn can_read_body(&self) -> bool {
        let can = match self.state {
            State::Body(Header { size, .. }) if self.buf.len() >= size as usize => true,
            _ => false,
        };
        trace!("can_read_body: {}", can);
        can
    }

    fn read_body(&mut self) -> Option<Message> {
        match self.state {
            State::Body(Header { ref log_type, size }) => {
                let bytes = self.buf.split_to(size as usize);
                let body = String::from_utf8(bytes.to_vec()).expect("bad vec");
                Some(match log_type {
                    &LogType::Stdin => Message::Stdin(body),
                    &LogType::Stdout => Message::Stdout(body),
                    &LogType::Stderr => Message::Stderr(body),
                })
            }
            State::Head => unreachable!(),
        }
    }
}

#[derive(Debug)]
pub enum Message {
    Stdout(String),
    Stderr(String),
    Stdin(String),
}

impl Stream for Logs {
    type Item = Message;
    type Error = DockerError;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        if self.finished {
            println!("body empty, finished");
            trace!("buf len: {}", self.buf.len());
            if self.buf.is_empty() {
                return Ok(Async::Ready(None));
            }
            if self.can_read_head() {
                let head = self.read_head().expect("can't read head");
                self.state = State::Body(head);
            }
            if self.can_read_body() {
                let body = self.read_body().expect("bad body");
                return Ok(Async::Ready(Some(body)));
            }
            return Ok(Async::Ready(None));
        }

        match self.body.poll() {
            Ok(Async::Ready(Some(chunk))) => {
                self.buf.extend(&*chunk);
                debug!("logs buf: {:?}", self.buf);
            }
            Err(_) => return Err(DockerError::UnknownError),
            Ok(Async::Ready(None)) => {
                trace!("body empty");
                self.finished = true;
                debug!("self.finished: {}", self.finished);
            }
            Ok(Async::NotReady) => {
                trace!("body not ready");
                ()
            }
        }

        if self.can_read_head() {
            let head = self.read_head().expect("bad head");
            trace!("head: {:?}", head);
            self.state = State::Body(head);
        }

        if self.can_read_body() {
            let body = self.read_body().expect("bad body");
            trace!("body: {:?}", body);
            return Ok(Async::Ready(Some(body)));
        }

        if self.finished && self.buf.is_empty() {
            Ok(Async::Ready(None))
        } else {
            Ok(Async::NotReady)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Header, LogType, Logs, Logs};
    use hyper::{Body, Chunk};
    use tokio_core::reactor::Core;
    use futures::Stream;
    use std::cmp::PartialEq;

    #[test]
    fn test_header_from_bytes() {
        let bytes = [1u8, 0, 0, 0, 0, 0, 2, 1];
        let header = Header::new(&bytes);
        assert_eq!(header.log_type, LogType::Stdout);
        assert_eq!(header.size, 0x0201);
    }

    #[test]
    fn test_message_create() {
        // TODO complete test
    }
}
