use bytes::BytesMut;
use futures::{Async, Future, Poll, Stream};
use hyper::client::Connect;
use hyper::header::{Header, Headers};
use hyper::{self, Method, Request, StatusCode};
use hyperlocal::Uri;
use json::{self, Deserializer as JsonDeserializer};
use url::form_urlencoded::Serializer as FormEncoder;

use futures::future;
use std::collections::HashMap;

use executor::client::Docker;
use executor::error::DockerError;

pub struct BuildMessages {
    body: hyper::Body,
    buf: BytesMut,
    finished: bool,
}

#[derive(Deserialize, Debug)]
pub struct Detail {
    code: i32,
    message: String,
}

/// Represents the types of messages that can be
/// deserialized from the build messages stream
#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum Message {
    /// Docker sends this type of message whenever an step completed
    Stream { stream: String },
    /// If an error is encountered at any step, this message is received
    ErrorDetail {
        #[serde(rename = "errorDetail")]
        error_detail: Detail,
        error: String,
    },
}

impl BuildMessages {
    pub fn new(body: hyper::Body) -> Self {
        BuildMessages {
            body: body,
            buf: BytesMut::with_capacity(64),
            finished: false,
        }
    }

    /// Returns the next message from `buf` if it contains one
    /// Also, removes the bytes used to construct the message from `buf`
    pub fn next_message(&mut self) -> Result<Option<Message>, json::Error> {
        let (next, byte_offset) = {
            let mut stream = JsonDeserializer::from_slice(&self.buf).into_iter::<Message>();
            let next = stream.next();
            (next, stream.byte_offset())
        };

        match next {
            Some(Ok(value)) => {
                self.buf = self.buf.split_off(byte_offset);
                Ok(Some(value))
            }
            Some(Err(e)) => {
                debug!("invalid stream: {:?}", self.buf);
                Err(e)
            }
            None => Ok(None),
        }
    }
}

impl Stream for BuildMessages {
    type Item = Message;
    type Error = hyper::Error;

    /// We are trying to read JSON Objects delimited by `\r\n`
    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        if self.finished {
            // last line has been parsed, we're done
            if self.buf == r"\r\n" {
                Ok(Async::Ready(None))
            } else {
                // read the list of messages next
                let next_message = self.next_message();
                if let Ok(Some(value)) = next_message {
                    Ok(Async::Ready(Some(value)))
                } else if let Err(_) = next_message {
                    Err(hyper::Error::Incomplete)
                } else {
                    Ok(Async::Ready(None))
                }
            }
        } else {
            match self.body.poll() {
                Ok(Async::NotReady) => return Ok(Async::NotReady),
                Ok(Async::Ready(Some(chunk))) => {
                    self.buf.extend(chunk);
                }
                Ok(Async::Ready(None)) => {
                    // inner stream exhausted
                    self.finished = true;
                }
                Err(e) => return Err(e),
            }
            let next_message = self.next_message();
            if let Ok(Some(value)) = next_message {
                Ok(Async::Ready(Some(value)))
            } else {
                // TODO: use task::notify() instead
                // not enough bytes available for a complete message
                if self.buf == "\r\n" {
                    Ok(Async::Ready(None))
                } else {
                    Ok(Async::NotReady)
                }
            }
        }
    }
}

/// Builder for construction Docker Images
pub struct ImageBuilder<T> {
    params: HashMap<String, String>,
    body: Option<T>,
    headers: Headers,
}

impl<T> ImageBuilder<T>
where
    T: Into<hyper::Body>,
{
    /// Instantiates an empty ImageBuilder
    pub fn new() -> Self {
        ImageBuilder {
            params: HashMap::new(),
            body: None,
            headers: Headers::new(),
        }
    }

    /// Sets the query params to be sent to Docker
    pub fn set_param(&mut self, key: &str, value: &str) {
        self.params.insert(key.to_owned(), value.to_owned());
    }

    /// Sets the Context to be sent to Docker for building an Image
    pub fn set_body(&mut self, body: T) {
        self.body = Some(body);
    }

    /// Sets the HTTP headers to be sent to Docker
    pub fn set_header<H: Header>(&mut self, header: H) {
        self.headers.set(header);
    }

    /// Sets the query params to be sent to Docker
    pub fn with_param(mut self, key: &str, value: &str) -> Self {
        self.set_param(key, value);
        self
    }

    #[allow(dead_code)]
    /// Sets the HTTP headers to be sent to Docker
    pub fn with_header<H: Header>(mut self, header: H) -> Self {
        self.set_header(header);
        self
    }

    /// Sets the Context to be sent to Docker for building an Image
    pub fn with_body(mut self, archive: T) -> Self {
        self.set_body(archive);
        self
    }

    /// Builds a HTTP Request to be sent to Docker
    pub fn build(self) -> Result<Request, DockerError> {
        let params = FormEncoder::new(String::new())
            .extend_pairs(self.params)
            .finish();
        let mut uri = String::from("/v1.30/build");
        if !params.is_empty() {
            uri.push_str(&"?");
            uri.push_str(&params);
        }
        let uri = Uri::new("/var/run/docker.sock", &uri);
        trace!("build params: {:?}", &uri);
        let mut request = Request::new(Method::Post, uri.into());
        if let Some(body) = self.body {
            request.set_body(body);
        }
        Ok(request)
    }

    pub fn build_on<C: Connect>(
        self,
        client: &Docker<C>,
    ) -> Box<Future<Item = BuildMessages, Error = DockerError>> {
        let request = match self.build() {
            Ok(request) => request,
            Err(_) => return Box::new(future::err(DockerError::BadRequest)),
        };
        let response = client.request(request).and_then(|resp| {
            trace!("image build status: {}", resp.status());
            match resp.status() {
                StatusCode::Ok => (),
                StatusCode::BadRequest => return future::err(DockerError::BadRequest),
                _ => return future::err(DockerError::InternalServerError),
            }
            future::ok(BuildMessages::new(resp.body()))
        });
        Box::new(response)
    }
}
