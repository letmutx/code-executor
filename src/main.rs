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

use futures::{future, Future};
use futures::Stream;

use docker::Docker;
use docker::Id;
use docker::Images;
use docker::ContainerBuilder;
use docker::ImageBuilder;

use tokio_core::reactor::{Remote, Core};
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
        builder.append_file(
            Path::new("Dockerfile"),
            &mut dockerfile,
        )?;
        builder.append(&header, self.code.as_bytes())?;
        let build = builder.into_inner()?;
        Ok(ImageBuilder::new().with_body(Body::from(build)))
    }
}

#[derive(Clone)]
struct Executor<T: Connect + Clone> {
    remote: Remote,
    client: Docker<T>,
    pool: CpuPool,
}

type ExecutionFuture = Box<Future<Item = hyper::Response, Error = ExecutionError>>;

fn split_at_colon<'a, T>(s: &'a str) -> Result<T, ()>
where
    T: From<(&'a str, &'a str)>,
{
    let index = s.find(|c| c == ':');
    match index {
        Some(index) => {
            let (first, rest) = s.split_at(index);
            let second = rest.chars().nth(1).unwrap();
            let c = rest.find(second);
            let (_, rest) = rest.split_at(c.unwrap());
            Ok(T::from((first, rest)))
        }
        None => Ok(T::from(("", ""))),
    }
}

enum ExecutionError {
    CompileError(String),
    RunTimeError,
    InternalError,
}

fn execute<T: Connect + Clone>(
    docker: Docker<T>,
    builder: ImageBuilder<Body>,
) -> Box<Future<Item = String, Error = ExecutionError>> {
    let image = Images::create_image_with(docker, builder);

    let container = image.map_err(|_| ExecutionError::InternalError).and_then(
        |(docker, build_steps)| {
            let error_message_holder = Vec::new();
            build_steps
                .map(|msg| match msg {
                    json::Value::Object(map) => map,
                    _ => panic!(),
                })
                .fold(error_message_holder, |mut holder, map| {
                    // TODO: something to extract compiler warning messages
                    if map.contains_key("stream") {
                        let message = match map.get("stream").unwrap() {
                            &json::Value::String(val) => val,
                            _ => panic!(),
                        };
                        if message.contains("Step") {
                            error_message_holder = Vec::new();
                        } else {
                            error_message_holder.push(message.to_owned());
                        }
                    } else if map.contains_key("errorDetail") {
                        let error_message = String::new();
                        for message in error_message_holder.into_iter().skip(1) {
                            error_message.push_str(&message);
                        }
                        return Err(ExecutionError::CompileError(error_message));
                    }
                    Ok(error_message_holder)
                })
                .and_then(|image_id| Ok(split_at_colon(&image_id[0]).unwrap().hash))
                .and_then(|id| {
                    let mut body = json::Map::new();
                    body.insert("Image".to_owned(), json::Value::String(id));
                    let mut builder = ContainerBuilder::new().with_body(body);
                    builder.set_header(ContentType::json());

                    let container = docker.create_container(builder);

                    container
                        .map_err(|_| ExecutionError::InternalError)
                        .and_then(|map| {
                            assert!(map.get("Id").is_some());
                            let container_id = map.get("Id").unwrap().to_owned();
                            if let json::Value::String(container_id) = container_id {
                                docker
                                    .start_container(&container_id)
                                    .map_err(|_| ExecutionError::InternalError)
                                    .and_then(|_| Ok((docker, container_id)))
                            } else {
                                panic!();
                            }
                        })
                })
        },
    );

    let logs = container.and_then(|(docker, container_id)| docker.logs(&container_id));

    let progress = logs.and_then(move |stream| {
        let output = String::new();
        println!("stream:: here");
        stream.map_err(|_| hyper::Error::Incomplete).fold(
            output,
            |mut output,
             message| {
                println!("here");
                output.push_str(&message);
                Ok::<_, hyper::Error>(output)
            },
        )
    });

    Box::new(progress)
}

fn send_request<T: Connect + Clone>(executor: Executor<T>, req: hyper::Request) -> ExecutionFuture {
    let body = Vec::new();
    let submission = req.body()
        .fold(body, move |mut body, chunk| {
            body.extend(chunk.into_iter());
            Ok::<_, hyper::Error>(body)
        })
        .map_err(|_err| hyper::Error::Status)
        .and_then(move |json| {
            let submission: Submission = json::from_slice(&json).unwrap();
            Ok(submission)
        });

    let pool = executor.pool.clone();
    let builder = submission.and_then(move |sub| {
        pool.spawn_fn(move || sub.construct_image_builder())
            .map_err(From::from)
    });
    let docker = executor.client.clone();
    let output = builder.and_then(|image_builder| execute(docker, image_builder));
    let response = output.and_then(|output| Ok(Response::new().with_body(Body::from(output))));
    Box::new(response)
}

impl<T: Connect + Clone> Service for Executor<T> {
    type Request = Request;
    type Response = Response;
    type Error = Error;
    type Future = ExecutionFuture;

    fn call(&self, req: Self::Request) -> Self::Future {
        match (req.method(), req.path()) {
            (&Method::Post, "/execute") => {
                let clone = (*self).clone();
                send_request(clone, req).then(|resp| {
                    futures::future::ok(match resp {
                        Ok(resp) => resp,
                        ExecutionError::CompileError(string) => Response::new().with_body(string),
                        ExecutionError::InternalError => {
                            Response::new().with_body("Internal Error")
                        }
                        ExecutionError::RunTimeError => Response::new().with_body("Runtime error"),
                    })
                })
            }
            _ => Box::new(futures::future::ok(
                Response::new().with_status(StatusCode::NotFound),
            )),
        }
    }
}

fn main() {
    let mut core = Core::new().unwrap();
    let handle = &core.handle();
    let remote = core.remote();
    let addr = "127.0.0.1:3000".parse().unwrap();
    let listener = TcpListener::bind(&addr, handle).unwrap();
    let client = Docker::<UnixConnector>::new(handle.clone());
    let pool = CpuPool::new(1);
    let service = listener.incoming().for_each(move |(socket, addr)| {
        let executor = Executor {
            client: client.clone(),
            remote: remote.clone(),
            pool: pool.clone(),
        };
        let server = Http::new().bind_connection(handle, socket, addr, executor);
        Ok(())
    });
    core.run(service).unwrap();
}
