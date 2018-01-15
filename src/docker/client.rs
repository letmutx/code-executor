use hyperlocal::{Uri, UnixConnector};
use hyper::{self, Body, Method, StatusCode};
use hyper::Client;
use hyper::client::{Request, Connect};
use tokio_core::reactor::Handle;
use json;
use docker::log::{Logs, LogMessage};
use docker::error::DockerError;

use docker::image::BuildMessages;

use futures::{future, Stream, Future, Poll};

pub struct Docker<C> {
    client: Client<C>,
}

type DockerResponse = Box<Future<Item = hyper::Response, Error = DockerError>>;

impl<C: Connect> Docker<C> {
    pub fn new(connector: C, handle: Handle) -> Docker<C> {
        let client = Client::configure()
            .connector(connector)
            .build(&handle);

        Docker { client: client }
    }

    pub fn request(&self, request: Request) -> Box<Future<Item = hyper::Response, Error = DockerError>> {
        let response = self.client.request(request)
            .map_err(|e| DockerError::HyperError(e));
        Box::new(response)
    }

    pub fn start_container(&self, id: &str) -> Box<Future<Item = (), Error = DockerError>> {
        let uri = format!("v1.30/containers/{id}/start", id = id);
        let uri = Uri::new("/var/run/docker.sock", &uri);
        let request = Request::new(Method::Post, uri.into());
        let resp = self.client.request(request)
            .map_err(|e| DockerError::HyperError(e))
            .and_then(|resp| {
                match resp.status() {
                    StatusCode::NoContent | StatusCode::NotModified => future::ok(()),
                    StatusCode::NotFound => future::err(DockerError::NotFound),
                    _ => future::err(DockerError::InternalServerError)
                }
            });
        Box::new(resp)
    }

    pub fn logs(&self, container_id: &str) -> Box<Future<Item = Logs, Error = DockerError>> {
        unimplemented!()
    }
}
