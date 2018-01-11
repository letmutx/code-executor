use hyperlocal::{Uri, UnixConnector};
use hyper::{self, Body, Method, StatusCode};
use hyper::Client;
use hyper::client::{Request, Connect};
use tokio_core::reactor::Handle;
use json;
use docker::log::{Logs, LogMessage};

use futures::{future, Stream, Future, Poll};

use docker::image::Images;
use docker::container::Containers;
use docker::image::{ImageBuilder, BuildMessage};
use docker::container::ContainerBuilder;
use docker::common::Id;

pub struct Docker<C = UnixConnector> {
    client: Client<C>,
}

impl<C: Connect> Docker<C> {
    pub fn new(handle: Handle) -> Docker {
        let client = Client::configure()
            .connector(UnixConnector::new(handle.clone()))
            .build(&handle);

        Docker { client: client }
    }

    pub fn images(&self) -> Images {
        let uri = Uri::new("/var/run/docker.sock", "/v1.30/images/json");
        let mut request = Request::new(Method::Get, uri.into());
        let result = self.client
            .request(request)
            .and_then(|resp| resp.body().concat2().map(|chunk| json::from_slice(&chunk).unwrap()));

        Images(Box::new(result))
    }

    pub fn create_image<T: Into<hyper::Body>>(&self, image: ImageBuilder<T>) -> Progress {
        let mut request = image.build();
        match request {
            Ok(_) => (),
            Err(e) => return Progress(Box::new(future::err(hyper::Error::Incomplete))),
        }
        let request = request.unwrap();
        let image_progress =
            self.client.request(request).and_then(|resp| Ok(BuildMessage::new(resp.body())));

        Progress(Box::new(image_progress))
    }

    pub fn create_image_quiet<T: Into<hyper::Body>>
        (&self,
         mut image: ImageBuilder<T>)
         -> Box<Future<Item = json::Map<String, json::Value>, Error = hyper::Error>> {
        image.set_param("q", "true");
        let request = image.build();
        match request {
            Ok(_) => (),
            Err(e) => return Box::new(future::err(hyper::Error::Incomplete)),
        }
        let request = request.unwrap();
        let json = self.client.request(request).and_then(|resp| {
            resp.body().concat2().map(|chunk| {
                println!("{:?}", &*chunk);
                json::from_slice(&chunk).unwrap()
            })
        });

        Box::new(json)
    }

    pub fn create_container
        (&self,
         container: ContainerBuilder)
         -> Box<Future<Item = json::Map<String, json::Value>, Error = hyper::Error>> {
        let request = container.build();
        match request {
            Ok(_) => (),
            Err(e) => return Box::new(future::err(hyper::Error::Incomplete)),
        }
        let request = request.unwrap();
        let container = self.client
            .request(request)
            .and_then(|resp| resp.body().concat2().map(|chunk| json::from_slice(&chunk).unwrap()));

        Box::new(container)
    }

    pub fn start_container(&self, id: &str) -> Box<Future<Item = (), Error = hyper::Error>> {
        println!("{}", id);
        let url = format!("/v1.30/containers/{}/start", id);
        let uri = Uri::new("/var/run/docker.sock", &url);
        let request = Request::new(Method::Post, uri.into());

        let resp = self.client.request(request).and_then(|resp| match resp.status() {
            StatusCode::NoContent => Ok(()),
            _ => panic!(),
        });

        Box::new(resp)
    }

    pub fn containers(&self) -> Containers {
        let uri = Uri::new("/var/run/docker.sock", "/v1.30/containers/json");
        let mut request = Request::new(Method::Get, uri.into());
        let result = self.client
            .request(request)
            .and_then(|resp| resp.body().concat2().map(|chunk| json::from_slice(&chunk).unwrap()));

        Containers(Box::new(result))
    }

    pub fn logs(&self, container_id: &str) -> Logs {
        let url = format!("/v1.30/containers/{}/logs?follow=true&stdout=true",
                          container_id);
        let uri = Uri::new("/var/run/docker.sock", &url);
        let request = Request::new(Method::Get, uri.into());
        let result = self.client.request(request).and_then(|resp| Ok(LogMessage::new(resp.body())));

        Logs(Box::new(result))
    }
}

pub struct Progress(pub Box<Future<Item = BuildMessage, Error = hyper::Error> + 'static>);

impl Future for Progress {
    type Item = BuildMessage;
    type Error = hyper::Error;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        self.0.poll()
    }
}

impl<C: Clone> Clone for Docker<C> {
    fn clone(&self) -> Docker<C> {
        Docker { client: self.client.clone() }
    }
}
