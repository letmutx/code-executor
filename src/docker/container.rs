use docker::common::{Id, Tag, Port};
use std::collections::HashMap;
use json::{self, Map};
use hyperlocal::Uri;
use url::form_urlencoded::Serializer as FormEncoder;

use hyper::{self, Method, Request};
use hyper::header::{Header, Headers};
use futures::{Poll, Future};

use docker::client::Docker;
use hyper::client::Connect;

#[derive(Serialize, Deserialize)]
pub struct Container {
    pub Created: u64,
    pub Command: String,
    pub State: String,
    pub Id: String,
    pub Image: Tag,
    pub ImageID: Id,
    pub Labels: HashMap<String, String>,
    pub NetworkSettings: json::Value,
    pub Names: Vec<String>,
    pub Ports: Vec<Port>,
    pub Status: String,
}

pub struct Containers(pub Box<Future<Item = Vec<Container>, Error = hyper::Error> + 'static>);

impl Future for Containers {
    type Item = Vec<Container>;
    type Error = hyper::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.0.poll()
    }
}

impl Containers {
    pub fn from_client<T: Connect + Clone>
        (client: Docker<T>)
         -> Box<Future<Item = (Docker<T>, Vec<Container>), Error = (Docker<T>, hyper::Error)>> {
        let clone = client.clone();
        let containers = client.containers()
            .and_then(|resp| Ok((client, resp)))
            .or_else(move |err| Err((clone, err)));
        Box::new(containers)
    }

    pub fn create_container_with<T>(client: Docker<T>,
                                    container_builder: ContainerBuilder)
                                    -> Box<Future<Item = (Docker<T>,
                                                          json::Map<String, json::Value>),
                                                  Error = (Docker<T>, hyper::Error)>>
        where T: Connect + Clone
    {
        let clone = client.clone();
        let container = client.create_container(container_builder)
            .map_err(|e| (clone, e))
            .and_then(|progress| Ok((client, progress)));
        Box::new(container)
    }
}

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

    pub fn with_param(mut self, key: &str, value: &str) -> Self {
        self.set_param(key, value);
        self
    }

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
}
