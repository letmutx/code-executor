use hyper::{Body, Chunk};
use hyper;
use bytes::BytesMut;
use bytes::{ByteOrder, BigEndian};
use std::iter::FromIterator;

use futures::{Future, Stream, Poll, Async};

pub struct LogMessage {
    body: Body,
    state: State,
    bytes: BytesMut,
}

enum State {
    Head,
    Body(u32),
}

#[derive(Debug, PartialEq)]
enum LogType {
    Stdout,
    Stdin,
    Stderr,
}

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
            _ => panic!(),
        };
        let size = BigEndian::read_u32(&bytes[4..]);
        Header {
            log_type: log_type,
            size: size,
        }
    }
}

impl LogMessage {
    pub fn new(body: Body) -> Self {
        LogMessage {
            body: body,
            state: State::Head,
            bytes: BytesMut::with_capacity(64),
        }
    }

    fn can_read_head(&self) -> bool {
        match self.state {
            State::Head if self.bytes.len() >= 8 => true,
            _ => false,
        }
    }

    fn read_head(&mut self) -> Option<Header> {
        match self.state {
            State::Head => {
                let bytes = self.bytes.split_to(8);
                Some(Header::new(&bytes))
            }
            _ => None,
        }
    }

    fn can_read_body(&self) -> bool {
        match self.state {
            State::Body(size) if self.bytes.len() >= size as usize => true,
            _ => false,
        }
    }

    fn read_body(&mut self) -> Option<String> {
        match self.state {
            State::Body(size) => {
                let bytes = self.bytes.split_to(size as usize);
                let body = String::from_utf8(bytes.to_vec()).unwrap();
                Some(body)
            }
            _ => unreachable!(),
        }
    }
}

pub struct Logs(pub Box<Future<Item = LogMessage, Error = hyper::Error>>);

impl Future for Logs {
    type Item = LogMessage;
    type Error = hyper::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.0.poll()
    }
}

impl Stream for LogMessage {
    type Item = String;
    type Error = hyper::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        let chunk = self.body.poll();
        match chunk {
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(Async::Ready(Some(chunk))) => {
                self.bytes.extend(&*chunk);
                if self.can_read_head() {
                    if let Some(header) = self.read_head() {
                        (*self).state = State::Body(header.size);
                    }
                }
                if self.can_read_body() {
                    if let Some(body) = self.read_body() {
                        (*self).state = State::Head;
                        return Ok(Async::Ready(Some(body)));
                    }
                }
                Ok(Async::NotReady)
            }
            Ok(Async::Ready(None)) => Ok(Async::Ready(None)),
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LogMessage, Logs, Header, LogType};
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
