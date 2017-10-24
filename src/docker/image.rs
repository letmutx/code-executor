use futures::{Future, Poll, Stream, Async};
use hyper::{self, Method, Request, Body};
use hyper::header::{Header, Headers};
use hyper::client::Connect;
use serde::de::{self, Visitor, Unexpected};
use serde::{Serializer, Deserializer, Deserialize};
use json::{self, Deserializer as JsonDeserializer};
use url::form_urlencoded::Serializer as FormEncoder;
use hyperlocal::{Uri, UnixConnector};

use std::collections::HashMap;
use std::fmt;
use std::fmt::Debug;

use docker::client::Docker;
use docker::client::Progress;
use docker::common::{Id, Tag};

#[derive(Serialize, Deserialize)]
pub struct Image {
    Containers: i32,
    Created: u64,
    Id: String,
    //        Labels: HashMap<String, >,
    ParentId: Id,
    //        repo_digests: Vec<>,
    RepoTags: Vec<Id>,
    SharedSize: i64,
    Size: u64,
    VirtualSize: u64,
}

pub struct Images(pub Box<Future<Item = Vec<Image>, Error = hyper::Error> + 'static>);

impl Images {
    pub fn from_client<T: Connect + Clone>(client: Docker<T>)
        -> Box<Future<Item = (Docker<T>, Vec<Image>), Error = (Docker<T>, hyper::Error)>> {
        let clone = client.clone();
        let images = client.images().and_then(|resp| Ok((client, resp)))
                                    .or_else(move |err| Err((clone, err)));
        Box::new(images)
    }

    pub fn create_image_with<T: Connect + Clone, B:Into<hyper::Body>>(client: Docker<T>, image_builder: ImageBuilder<B>)
            -> Box<Future<Item = (Docker<T>, ImageBuildStep), Error = (Docker<T>, hyper::Error)>> {
        let clone = client.clone();
        let image = client.create_image(image_builder)
                          .map_err(|e| (clone, e))
                          .and_then(|progress| Ok((client, progress)));
        Box::new(image)
    }

    pub fn create_image_quietly_with<T, B>(client: Docker<T>, image_builder: ImageBuilder<B>)
            -> Box<Future<Item = (Docker<T>, json::Map<String, json::Value>), Error = (Docker<T>, hyper::Error)>>
            where T: Connect + Clone,
                  B: Into<hyper::Body> {
        let clone = client.clone();
        let image = client.create_image_quiet(image_builder)
                          .map_err(|e| (clone, e))
                          .and_then(|progress| Ok((client, progress)));
        Box::new(image)
    }
}

impl Future for Images {
    type Item = Vec<Image>;
    type Error = hyper::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.0.poll()
    }
}

pub struct ImageBuildStep {
    body: hyper::Body,
    remaining: Vec<u8>,
    finished: bool
}

impl ImageBuildStep {
    pub fn new(body: hyper::Body) -> Self {
        ImageBuildStep {
            body: body,
            remaining: Vec::new(),
            finished: false
        }
    }
}

impl Stream for ImageBuildStep {
    type Item = Result<json::Value, json::Error>;
    type Error = hyper::Error;

    fn poll(&mut self) -> Poll<Option<Self::Item>, Self::Error> {
        if self.finished {
            return Ok(Async::Ready(None));
        }
        let value = self.body.poll();
        match value {
            Ok(Async::NotReady) => Ok(Async::NotReady),
            Ok(Async::Ready(Some(chunk))) => {
                self.remaining.extend(chunk);
                let (next, byte_offset) = {
                    let mut stream = JsonDeserializer::from_slice(&self.remaining).into_iter::<json::Value>();
                    let next = stream.next();
                    (next, stream.byte_offset())
                };
                if next.is_some() {
                    self.remaining = self.remaining.split_off(byte_offset);
                    Ok(Async::Ready(next))
                } else {
                    Ok(Async::NotReady)
                }
            },
            Ok(Async::Ready(None)) => {
                self.finished = true;
                if !self.remaining.is_empty() {
                    let mut stream = json::Deserializer::from_slice(&self.remaining).into_iter::<json::Value>();
                    let result = stream.next();
                    Ok(Async::Ready(result))
                } else {
                    Ok(Async::NotReady)
                }
            }
            Err(e) => Err(e)
        }
    }
}

pub enum BuilderError {
    BadParams,
}

impl Debug for BuilderError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        write!(formatter, "{:?}", *self)
    }
}

pub struct ImageBuilder<T: Into<hyper::Body>> {
    params: HashMap<String, String>,
    body: Option<T>,
    headers: Headers,
}

impl<T: Into<hyper::Body>> ImageBuilder<T> {
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

    pub fn with_header<H: Header>(mut self, header: H) -> Self {
        self.set_header(header);
        self
    }

    pub fn with_body(mut self, archive: T) -> Self {
        self.set_body(archive);
        self
    }

    pub fn build(self) -> Result<Request, BuilderError> {
        let params = FormEncoder::new(String::new()).extend_pairs(self.params).finish();
        let mut uri = String::from("/v1.30/build");
        if !params.is_empty() {
            uri.push_str(&"?");
            uri.push_str(&params);
        }
        let uri = Uri::new("/var/run/docker.sock", &uri);
        let mut request = Request::new(Method::Post, uri.into());
        if let Some(body) = self.body {
            request.set_body(body);
        }
        Ok(request)
    }
}

