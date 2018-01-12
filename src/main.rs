#![allow(dead_code)]
#![allow(unused_imports)]

extern crate tokio_core;
extern crate futures;
extern crate futures_cpupool as cpupool;
extern crate hyper;
extern crate hyperlocal;
extern crate serde;
extern crate tar;
#[macro_use]
extern crate serde_derive;
extern crate url;
#[macro_use]
extern crate serde_json as json;
extern crate bytes;

mod docker;

use hyper::server::Http;
use hyper::server::Service;
use hyper::server::{Request, Response};
use hyper::client::{Client, Connect};
use hyperlocal::UnixConnector;
use hyper::{Method, StatusCode, Body, Error, Chunk};
use hyper::header::ContentType;

use tar::{Builder, Header};

use cpupool::CpuPool;
use std::rc::Rc;

use futures::{future, Future};
use futures::Stream;

use docker::Docker;
use docker::Id;
use docker::Images;
use docker::ContainerBuilder;
use docker::ImageBuilder;

use tokio_core::reactor::{Core, Handle};
use tokio_core::net::TcpListener;

use std::io::{self, ErrorKind};
use std::fs::File;
use std::path::Path;
use std::clone::Clone;


#[derive(Serialize, Deserialize, Debug)]
struct Submission {
    code: String,
    lang: Language,
}

const DOCKERFILE: &'static str = "resources/Dockerfile";

#[derive(Serialize, Deserialize, Debug)]
enum Language {
    C,
}

impl Submission {
    fn construct_image_builder(&self) -> Result<ImageBuilder<Body>, io::Error> {
        let mut builder = Builder::new(Vec::new());
        let mut dockerfile = File::open(DOCKERFILE)?;
        let mut header = Header::new_gnu();
        header.set_path("code.c");
        header.set_size(self.code.bytes().len() as u64);
        header.set_cksum();
        builder.append_file(Path::new("Dockerfile"), &mut dockerfile)?;
        builder.append(&header, self.code.as_bytes())?;
        let build = builder.into_inner()?;
        Ok(ImageBuilder::new().with_body(Body::from(build)))
    }
}

type Stdin = String;
type Stderr = String;

#[derive(Serialize)]
enum Output {
    CompileError(String),
    Output(i32, Stdin, Stderr),
}

enum ExecutionError {}

#[derive(Clone)]
struct Executor<C> {
    docker: Rc<Docker<C>>,
    pool: CpuPool,
}

impl<C: Connect> Executor<C> {
    fn new(connector: C, handle: Handle) -> Self {
        Executor {
            docker: Rc::new(Docker::new(connector, handle)),
            pool: CpuPool::new(1),
        }
    }
}

impl<C: Connect> Service for Executor<C> {
    type Request = Submission;
    type Response = Output;
    type Error = ExecutionError;
    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, sub: Self::Request) -> Self::Future {
        unimplemented!();
    }
}

struct APIService<E> {
    executor: E,
}

impl<E> APIService<E> {
    fn new(executor: E) -> Self {
        APIService { executor: executor }
    }
}

enum APIError {
    BadRequest,
    HyperError,
    ExecutionError,
}

impl<E> Service for APIService<E>
where E: Clone + Service<Request = Submission, Response = Output, Error = ExecutionError> + 'static
{
    type Request = hyper::server::Request;
    type Response = hyper::server::Response;
    type Error = hyper::Error;
    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, req: Self::Request) -> Self::Future {
        match (req.method(), req.path()) {
            (&Method::Post, "/execute") => {
                let executor = self.executor.clone();
                let response = req.body()
                    .fold(Vec::new(), |mut body, chunk| {
                        body.extend(chunk.into_iter());
                        Ok::<_, hyper::Error>(body)
                    })
                .map_err(|_| APIError::HyperError)
                    .and_then(|json| match json::from_slice::<Submission>(&json) {
                        Ok(json) => future::ok(json),
                        _ => future::err(APIError::BadRequest),
                    })
                .and_then(move |sub: Submission| {
                    executor.call(sub)
                        .map_err(|_| APIError::ExecutionError)
                        .and_then(|resp| {
                            Ok(Response::new()
                               .with_body(Body::from(json::to_string(&resp).unwrap())))
                        })
                })
                .then(|result| {
                    let response = match result {
                        Ok(response) => response,
                        Err(APIError::BadRequest) => {
                            Response::new()
                                .with_body(Body::from("Invalid json"))
                                .with_status(StatusCode::BadRequest)
                        }
                        _ => Response::new().with_body(Body::from("Unknown error")),
                    };
                    Ok(response)
                });
                Box::new(response)
            }
            _ => {
                Box::new(future::ok(Response::new()
                                    .with_body(Body::from("Invalid URL"))
                                    .with_status(StatusCode::NotFound)))
            }
        }
    }
}

fn main() {
    let mut core = Core::new().unwrap();
    let handle = &core.handle();
    let remote = core.remote();
    let addr = "127.0.0.1:3000".parse().unwrap();
    let listener = TcpListener::bind(&addr, handle).unwrap();
    let pool = CpuPool::new(1);
    let executor = Executor::new(UnixConnector::new(handle.clone()), handle.clone());
    let service = listener.incoming().for_each(move |(socket, addr)| {
        let api_service = APIService::new(executor.clone());
        let server = Http::new().bind_connection(handle, socket, addr, api_service);
        Ok(())
    });
    core.run(service).unwrap();
}
