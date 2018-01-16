extern crate bytes;
extern crate futures;
extern crate futures_cpupool as cpupool;
extern crate hyper;
extern crate hyperlocal;
extern crate serde;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json as json;
extern crate tar;
extern crate tokio_core;
extern crate url;

mod docker;
use docker::DockerError;

use hyper::server::Http;
use hyper::server::Service;
use hyper::server::Response;
use hyper::client::Connect;
use hyperlocal::UnixConnector;
use hyper::{Body, Method, StatusCode};
use hyper::header::ContentType;

use tar::{Builder, Header};

use cpupool::CpuPool;
use std::rc::Rc;

use futures::{future, Future};
use futures::Stream;

use docker::Docker;
use docker::log;
use docker::ContainerBuilder;
use docker::{ImageBuilder, Message};

use tokio_core::reactor::{Core, Handle};
use tokio_core::net::TcpListener;

use std::iter::Iterator;
use std::fs::File;
use std::path::Path;
use std::clone::Clone;

#[derive(Serialize, Deserialize, Debug)]
struct Submission {
    code: String,
    lang: Language,
}

const DOCKERFILE: &'static str = "resources/Dockerfile";

#[derive(Serialize, Deserialize, Debug, Copy, Clone)]
enum Language {
    C,
}

type Stdin = String;
type Stderr = String;

#[derive(Serialize)]
enum Output {
    CompileError(String),
    Output(Stdin, Stderr),
}

enum ExecutionError {
    BadConfig,
    DockerError(DockerError),
    CompileError(String),
    UnknownError,
}

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

enum Transform {
    Id(String),
    Error(String),
    Empty,
}

impl<C: Connect> Service for Executor<C> {
    type Request = Submission;
    type Response = Output;
    type Error = ExecutionError;
    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, sub: Self::Request) -> Self::Future {
        let tar = self.pool.spawn_fn(move || build_tar(sub));
        let client = self.docker.clone();
        let client2 = client.clone();
        let image = tar.map_err(|_| ExecutionError::BadConfig)
            .and_then(move |tar| {
                ImageBuilder::new()
                    .with_body(tar)
                    .build_on(&client)
                    .map_err(|e| ExecutionError::DockerError(e))
            });
        let logs = image.and_then(move |messages| {
            messages
                .map_err(|e| ExecutionError::DockerError(DockerError::HyperError(e)))
                .fold(Transform::Empty, |last_step, mut msg| match msg {
                    Message::Stream { ref mut stream } if stream.starts_with("sha256") => {
                        let id = stream.split(":").skip(1).next().unwrap().to_owned();
                        future::ok(Transform::Id(id))
                    }
                    Message::Stream { ref mut stream } if stream.contains("Step") => {
                        match last_step {
                            Transform::Id(msg) => future::ok(Transform::Id(msg)),
                            _ => future::ok(Transform::Empty),
                        }
                    }
                    Message::Stream { mut stream } => match last_step {
                        Transform::Id(msg) => future::ok(Transform::Id(msg)),
                        Transform::Empty => future::ok(Transform::Error(stream)),
                        Transform::Error(msg) => {
                            stream.push_str(&msg);
                            future::ok(Transform::Error(msg))
                        }
                    },
                    Message::ErrorDetail { .. } => future::ok(last_step),
                })
                .and_then(|msg| match msg {
                    Transform::Error(msg) => future::err(ExecutionError::CompileError(msg)),
                    Transform::Empty => future::err(ExecutionError::UnknownError),
                    Transform::Id(id) => future::ok(id),
                })
                .and_then(move |id| {
                    let config = json!({
                        "NetworkDisabled": true,
                        "Image": id
                    });
                    ContainerBuilder::new()
                        .with_body(config.as_object().unwrap().clone())
                        .with_header(ContentType::json())
                        .build_on(&client2)
                        .map_err(|_| ExecutionError::UnknownError)
                        .map(|id| (client2, id))
                })
                .and_then(move |(client, id)| {
                    client
                        .start_container(&id)
                        .map_err(|_| ExecutionError::UnknownError)
                        .and_then(|_| Ok((client, id)))
                })
                .and_then(|(client, id)| {
                    client
                        .logs(&id)
                        .map_err(|_| ExecutionError::UnknownError)
                        .and_then(|logs| {
                            logs.map_err(|_| ExecutionError::UnknownError)
                                .fold(
                                    (String::from(""), String::from("")),
                                    |(mut stdout, mut stderr), msg| {
                                        match msg {
                                            log::Message::Stdout(msg) => {
                                                stdout.push_str(&msg);
                                            }
                                            log::Message::Stderr(msg) => {
                                                stderr.push_str(&msg);
                                            }
                                            _ => (),
                                        }
                                        Ok((stdout, stderr))
                                    },
                                )
                                .and_then(|(stdout, stderr)| Ok(Output::Output(stdout, stderr)))
                        })
                })
                .then(|result| match result {
                    Ok(output) => future::ok(output),
                    Err(ExecutionError::CompileError(msg)) => future::ok(Output::CompileError(msg)),
                    Err(e) => future::err(e),
                })
        });
        Box::new(logs)
    }
}

fn build_tar(sub: Submission) -> Result<Vec<u8>, ::std::io::Error> {
    let mut builder = Builder::new(Vec::new());
    let mut dockerfile = File::open(DOCKERFILE)?;
    let mut header = Header::new_gnu();
    header.set_path("code.c")?;
    header.set_size(sub.code.bytes().len() as u64);
    header.set_cksum();
    builder.append_file(Path::new("Dockerfile"), &mut dockerfile)?;
    builder.append(&header, sub.code.as_bytes())?;
    builder.into_inner()
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
where
    E: Clone + Service<Request = Submission, Response = Output, Error = ExecutionError> + 'static,
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
                        executor
                            .call(sub)
                            .map_err(|_| APIError::ExecutionError)
                            .and_then(|resp| {
                                Ok(Response::new()
                                    .with_body(Body::from(json::to_string(&resp).unwrap())))
                            })
                    })
                    .then(|result| {
                        let response = match result {
                            Ok(response) => response,
                            Err(APIError::BadRequest) => Response::new()
                                .with_body(Body::from("Invalid json"))
                                .with_status(StatusCode::BadRequest),
                            _ => Response::new().with_body(Body::from("Unknown error")),
                        };
                        Ok(response)
                    });
                Box::new(response)
            }
            _ => Box::new(future::ok(
                Response::new()
                    .with_body(Body::from("Invalid URL"))
                    .with_status(StatusCode::NotFound),
            )),
        }
    }
}

fn main() {
    let mut core = Core::new().unwrap();
    let handle = &core.handle();
    let addr = "127.0.0.1:3000".parse().unwrap();
    let listener = TcpListener::bind(&addr, handle).unwrap();
    let executor = Executor::new(UnixConnector::new(handle.clone()), handle.clone());
    let service = listener.incoming().for_each(move |(socket, addr)| {
        let api_service = APIService::new(executor.clone());
        let _ = Http::new().bind_connection(handle, socket, addr, api_service);
        Ok(())
    });
    core.run(service).unwrap();
}
