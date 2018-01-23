use hyper::Body;
use bytes::BytesMut;
use bytes::{BigEndian, ByteOrder};

use docker::DockerError;

use futures::{Async, Poll, Stream};
use futures::stream::Fuse;

pub struct Logs {
    body: Fuse<Body>,
    state: State,
    buf: BytesMut,
}

#[derive(Debug)]
enum State {
    Head,
    Body(Header),
}

#[derive(Debug, PartialEq, Copy, Clone)]
enum LogType {
    Stdout,
    Stdin,
    Stderr,
}

#[derive(Debug, Copy, Clone)]
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
            body: body.fuse(),
            state: State::Head,
            buf: BytesMut::with_capacity(64),
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
        loop {
            trace!("state: {:?}", self.state);

            let mut finished = false;
            let mut not_ready = false;
            match self.body.poll() {
                Ok(Async::NotReady) => not_ready = true,
                Ok(Async::Ready(Some(chunk))) => {
                    self.buf.extend(chunk);
                }
                Ok(Async::Ready(None)) => finished = true,
                Err(_) => return Err(DockerError::UnknownError),
            }

            trace!("buf len: {}, finished: {}", self.buf.len(), finished);
            match self.state {
                State::Head => {
                    if finished {
                        debug_assert!(self.buf.is_empty());
                        return Ok(Async::Ready(None));
                    }
                    if not_ready {
                        return Ok(Async::NotReady);
                    }
                    if self.buf.len() < 8 {
                        continue;
                    }
                    let buf = self.buf.split_to(8);
                    let header = Header::new(&buf);
                    self.state = State::Body(header);
                }
                State::Body(Header { log_type, size }) => {
                    if self.buf.len() >= size as usize {
                        let bytes = self.buf.split_to(size as usize);
                        // TODO: not necessarily valid string
                        let string = String::from_utf8(bytes.to_vec()).expect("Bad bytes");
                        let message = match log_type {
                            LogType::Stdout => Message::Stdout(string),
                            LogType::Stdin => Message::Stdin(string),
                            LogType::Stderr => Message::Stderr(string),
                        };
                        self.state = State::Head;
                        return Ok(Async::Ready(Some(message)));
                    }
                    if not_ready {
                        return Ok(Async::NotReady);
                    }
                }
            }
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
