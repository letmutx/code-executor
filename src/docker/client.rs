use hyperlocal::Uri;
use hyper::{self, Method, StatusCode};
use hyper::Client;
use hyper::client::{Connect, Request};
use tokio_core::reactor::Handle;
use docker::log::Logs;
use docker::error::DockerError;

use std::collections::HashMap;
use url::form_urlencoded::Serializer as FormEncoder;

use futures::{future, Future};

pub struct Docker<C> {
    client: Client<C>,
}

type DockerResponse = Box<Future<Item = hyper::Response, Error = DockerError>>;

impl<C: Connect> Docker<C> {
    pub fn new(connector: C, handle: Handle) -> Docker<C> {
        let client = Client::configure().connector(connector).build(&handle);

        Docker { client: client }
    }

    pub fn request(&self, request: Request) -> DockerResponse {
        let response = self.client
            .request(request)
            .map_err(|e| DockerError::HyperError(e));
        Box::new(response)
    }

    pub fn start_container(&self, id: &str) -> Box<Future<Item = (), Error = DockerError>> {
        let uri = format!("v1.30/containers/{id}/start", id = id);
        let uri = Uri::new("/var/run/docker.sock", &uri);
        let request = Request::new(Method::Post, uri.into());
        let resp = self.client
            .request(request)
            .map_err(|e| DockerError::HyperError(e))
            .and_then(|resp| match resp.status() {
                StatusCode::NoContent | StatusCode::NotModified => future::ok(()),
                StatusCode::NotFound => future::err(DockerError::NotFound),
                _ => future::err(DockerError::InternalServerError),
            });
        Box::new(resp)
    }

    pub fn logs(&self, container_id: &str) -> Box<Future<Item = Logs, Error = DockerError>> {
        let mut params = HashMap::new();
        params.insert("follow", "true");
        params.insert("stdout", "true");
        params.insert("stderr", "true");
        let params = FormEncoder::new(String::new())
            .extend_pairs(params)
            .finish();
        let mut uri = format!("v1.30/containers/{id}/logs?", id = container_id);
        uri.push_str(&params);
        let uri = Uri::new("/var/run/docker.sock", &uri);
        let request = Request::new(Method::Get, uri.into());
        let response = self.request(request).and_then(|resp| {
            match resp.status() {
                StatusCode::SwitchingProtocols => (),
                StatusCode::NotFound => return future::err(DockerError::NotFound),
                _ => return future::err(DockerError::InternalServerError),
            }
            future::ok(Logs::new(resp.body()))
        });

        Box::new(response)
    }
}
