use bytes::BytesMut;
use bytes::{BigEndian, ByteOrder};
use hyper::Body;

use executor::DockerError;

use futures::stream::Fuse;
use futures::{Async, Poll, Stream};

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

/// Header of the log frame
#[derive(Debug, Copy, Clone)]
struct Header {
    pub log_type: LogType,
    pub size: u32,
}

impl Header {
    /// Create a header
    /// # Arguments:
    /// * `bytes` - Should be atleast 8 bytes long
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
    /// Create a new `Logs` instance from a body
    pub fn new(body: Body) -> Self {
        Logs {
            body: body.fuse(),
            state: State::Head,
            buf: BytesMut::with_capacity(64),
        }
    }
}

/// Body of the log frame
#[derive(Debug)]
pub enum Message {
    Stdout(String),
    Stderr(String),
    Stdin(String),
}

impl Stream for Logs {
    type Item = Message;
    type Error = DockerError;

    /// The inner stream has frames with an 8-byte header
    /// The first byte of the header denotes the type of the body
    /// The next three bytes are unused, the remaining four bytes
    /// encoded in big-endian format consist of a u32 which is the
    /// size of the body
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
                        let len = self.buf.len();
                        if len == 0 {
                            return Ok(Async::Ready(None));
                        } else if len > 8 {
                            let buf = self.buf.split_to(8);
                            let header = Header::new(&buf);
                            self.state = State::Body(header);
                            continue;
                        } else {
                            debug!("self.buf {:?}", self.buf);
                            return Err(DockerError::UnknownError);
                        }
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
                        // FIXME: not necessarily valid string
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
