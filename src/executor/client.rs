use executor::error::DockerError;
use executor::log::Logs;
use hyper::Client;
use hyper::client::{Connect, Request};
use hyper::header::{Connection, ConnectionOption};
use hyper::{self, Method, StatusCode};
use hyperlocal::Uri;
use tokio_core::reactor::Handle;
use unicase::Ascii;

use std::collections::HashMap;
use url::form_urlencoded::Serializer as FormEncoder;

use futures::{future, Future};

/// Docker Client
pub struct Docker<C> {
    client: Client<C>,
}

type DockerResponse = Box<Future<Item = hyper::Response, Error = DockerError>>;

impl<C: Connect> Docker<C> {
    /// Creates a new Docker Client connected over the `connector`
    /// It is tied to an event loop by the `Handle`
    pub fn new(connector: C, handle: Handle) -> Docker<C> {
        let client = Client::configure().connector(connector).build(&handle);

        Docker { client: client }
    }

    /// Helper method for sending requests which don't
    /// have any high level wrappers or builders
    pub fn request(&self, request: Request) -> DockerResponse {
        let response = self.client
            .request(request)
            .map_err(|e| DockerError::HyperError(e));
        Box::new(response)
    }

    /// Starts a container specified by the `id`
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

    /// Returns logs from the container specified by `container_id`
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
        trace!("{}", uri);
        let uri = Uri::new("/var/run/docker.sock", &uri);
        let mut request = Request::new(Method::Get, uri.into());
        let upgrade = Connection(vec![
            ConnectionOption::ConnectionHeader(Ascii::new("upgrade".to_owned())),
        ]);
        request.headers_mut().set(upgrade);
        let response = self.request(request).and_then(|resp| {
            trace!("logs status: {}", resp.status());
            match resp.status() {
                StatusCode::Ok | StatusCode::SwitchingProtocols => (),
                StatusCode::NotFound => return future::err(DockerError::NotFound),
                _ => return future::err(DockerError::InternalServerError),
            }
            future::ok(Logs::new(resp.body()))
        });

        Box::new(response)
    }
}
