use std::collections::HashMap;
use json::{self, Map};
use hyperlocal::Uri;
use url::form_urlencoded::Serializer as FormEncoder;

use hyper::{self, Method, Request};
use hyper::header::{Header, Headers};
use futures::{future, Future};
use futures::Stream;

use executor::error::DockerError;
use executor::client::Docker;
use hyper::client::Connect;
use hyper::StatusCode;

pub struct ContainerBuilder {
    params: HashMap<String, String>,
    body: Map<String, json::Value>,
    headers: Headers,
}

impl ContainerBuilder {
    pub fn new() -> Self {
        ContainerBuilder {
            params: HashMap::new(),
            body: Map::new(),
            headers: Headers::new(),
        }
    }

    pub fn set_param(&mut self, key: &str, value: &str) {
        self.params.insert(key.to_owned(), value.to_owned());
    }

    pub fn set_body(&mut self, body: json::Map<String, json::Value>) {
        self.body = body;
    }

    pub fn set_header<H: Header>(&mut self, header: H) {
        self.headers.set(header);
    }

    #[allow(dead_code)]
    pub fn with_param(mut self, key: &str, value: &str) -> Self {
        self.set_param(key, value);
        self
    }

    #[allow(dead_code)]
    pub fn with_header<H: Header>(mut self, header: H) -> Self {
        self.set_header(header);
        self
    }

    pub fn with_body(mut self, body: json::Map<String, json::Value>) -> Self {
        self.set_body(body);
        self
    }

    pub fn build(self) -> Result<Request, hyper::Error> {
        let params = FormEncoder::new(String::new())
            .extend_pairs(self.params)
            .finish();
        let mut uri = String::from("/v1.30/containers/create");
        if !params.is_empty() {
            uri.push_str(&"?");
            uri.push_str(&params);
        }
        let uri = Uri::new("/var/run/docker.sock", &uri);
        let mut req = Request::new(Method::Post, uri.into());

        *req.headers_mut() = self.headers;
        if self.body.len() > 0 {
            req.set_body(json::to_string(&self.body).unwrap());
        }
        Ok(req)
    }

    pub fn build_on<C: Connect>(
        self,
        client: &Docker<C>,
    ) -> Box<Future<Item = String, Error = DockerError>> {
        let request = match self.build() {
            Ok(request) => request,
            _ => return Box::new(future::err(DockerError::BadRequest)),
        };
        let response = client.request(request).and_then(|resp| {
            let status = resp.status();
            resp.body()
                .map_err(|e| DockerError::HyperError(e))
                .fold(Vec::new(), |mut body, chunk| {
                    body.extend(&*chunk);
                    Ok(body)
                })
                .and_then(move |body| {
                    debug!("container request body: {:?}, status: {}", body, status);
                    match json::from_slice(&body) {
                        Ok(json::Value::Object(map)) => match status {
                            StatusCode::Created => future::ok(
                                map.get("Id")
                                    .expect("expected id")
                                    .as_str()
                                    .unwrap()
                                    .to_owned(),
                            ),
                            StatusCode::NotAcceptable => future::err(DockerError::CantAttach),
                            StatusCode::NotFound | StatusCode::BadRequest => {
                                future::err(DockerError::BadRequest)
                            }
                            _ => future::err(DockerError::InternalServerError),
                        },
                        _ => future::err(DockerError::UnknownError),
                    }
                })
        });
        Box::new(response)
    }
}
