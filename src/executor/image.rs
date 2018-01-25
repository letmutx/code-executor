use futures::{Async, Future, Poll, Stream};
use hyper::{self, Method, Request, StatusCode};
use hyper::header::{Header, Headers};
use hyper::client::Connect;
use json::{self, Deserializer as JsonDeserializer};
use url::form_urlencoded::Serializer as FormEncoder;
use hyperlocal::Uri;
use bytes::BytesMut;

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

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum Message {
    Stream {
        stream: String,
    },
    ErrorDetail {
        #[serde(rename = "errorDetail")] error_detail: Detail,
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

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        if self.finished {
            if self.buf == r"\r\n" {
                Ok(Async::Ready(None))
            } else {
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
                    self.finished = true;
                }
                Err(e) => return Err(e),
            }
            let next_message = self.next_message();
            if let Ok(Some(value)) = next_message {
                Ok(Async::Ready(Some(value)))
            } else {
                // TODO: use task::notify() instead
                if self.buf == "\r\n" {
                    Ok(Async::Ready(None))
                } else {
                    Ok(Async::NotReady)
                }
            }
        }
    }
}

pub struct ImageBuilder<T> {
    params: HashMap<String, String>,
    body: Option<T>,
    headers: Headers,
}

impl<T> ImageBuilder<T>
where
    T: Into<hyper::Body>,
{
    pub fn new() -> Self {
        ImageBuilder {
            params: HashMap::new(),
            body: None,
            headers: Headers::new(),
        }
    }

    pub fn set_param(&mut self, key: &str, value: &str) {
        self.params.insert(key.to_owned(), value.to_owned());
    }

    pub fn set_body(&mut self, body: T) {
        self.body = Some(body);
    }

    pub fn set_header<H: Header>(&mut self, header: H) {
        self.headers.set(header);
    }

    pub fn with_param(mut self, key: &str, value: &str) -> Self {
        self.set_param(key, value);
        self
    }

    #[allow(dead_code)]
    pub fn with_header<H: Header>(mut self, header: H) -> Self {
        self.set_header(header);
        self
    }

    pub fn with_body(mut self, archive: T) -> Self {
        self.set_body(archive);
        self
    }

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
