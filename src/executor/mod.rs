mod error;
mod client;
mod container;
mod image;
mod log;

use self::error::DockerError;
use self::client::Docker;
use self::image::{ImageBuilder, Message};
use self::container::ContainerBuilder;
use hyper::header::ContentType;
use hyper::client::Connect;
use hyper::server::Service;
use tokio_core::reactor::Handle;

use futures::{Future, Stream};

use tar::{Builder, Header};

use Output;
use Submission;
use Language;

use cpupool::CpuPool;
use futures::future;

use std::fs::File;
use std::path::Path;
use std::rc::Rc;

fn build_tar(sub: Submission) -> Result<Vec<u8>, ::std::io::Error> {
    let mut builder = Builder::new(Vec::new());
    let mut dockerfile = File::open(sub.lang.get_docker_file())?;
    let mut header = Header::new_gnu();
    header.set_path(sub.lang.get_file_name())?;
    header.set_size(sub.code.bytes().len() as u64);
    header.set_cksum();
    builder.append_file(Path::new("Dockerfile"), &mut dockerfile)?;
    builder.append(&header, sub.code.as_bytes())?;
    builder.into_inner()
}

trait LanguageConfig {
    fn get_file_name(&self) -> &'static str;
    fn get_docker_file(&self) -> &'static str;
}

impl LanguageConfig for Language {
    fn get_file_name(&self) -> &'static str {
        match *self {
            Language::C => "code.c",
            Language::Python2 => "code.py",
        }
    }

    fn get_docker_file(&self) -> &'static str {
        match *self {
            Language::C => "resources/c/Dockerfile",
            Language::Python2 => "resources/python2/Dockerfile",
        }
    }
}

#[derive(Debug)]
pub enum ExecutionError {
    BadConfig,
    DockerError(DockerError),
    CompileError(String),
    UnknownError,
}

#[derive(Clone)]
pub struct Executor<C> {
    docker: Rc<Docker<C>>,
    pool: CpuPool,
}

impl<C: Connect> Executor<C> {
    pub fn new(connector: C, handle: Handle) -> Self {
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
        trace!("executor called: {:?}", sub);
        let tar = self.pool.spawn_fn(move || build_tar(sub));
        let client = self.docker.clone();
        let client2 = client.clone();
        let image = tar.map_err(|e| {
            debug!("can't create tar: {:?}", e);
            ExecutionError::BadConfig
        }).and_then(move |tar| {
            trace!("building image");
            ImageBuilder::new()
                .with_body(tar)
                .with_param("q", "true")
                .build_on(&client)
                .map_err(|e| {
                    debug!("error: {:?}", e);
                    ExecutionError::DockerError(e)
                })
        });
        let logs = image.and_then(move |messages| {
            messages
                .map_err(|e| {
                    debug!("error: {:?}", e);
                    ExecutionError::DockerError(DockerError::HyperError(e))
                })
                .fold(Transform::Empty, |last_step, mut msg| {
                    debug!("build message: {:?}", msg);
                    // TODO: remove terminal coloring sequences
                    match msg {
                        Message::Stream { ref mut stream } if stream.starts_with("sha256") => {
                            let id = stream
                                .split(":")
                                .skip(1)
                                .next()
                                .unwrap()
                                .trim_right()
                                .to_owned();
                            future::ok(Transform::Id(id))
                        }
                        Message::Stream { ref mut stream } if stream.contains("Step") => {
                            match last_step {
                                Transform::Id(msg) => future::ok(Transform::Id(msg)),
                                _ => future::ok(Transform::Empty),
                            }
                        }
                        // cache messages - no thank you :|
                        Message::Stream { ref stream } if stream.contains("---") => {
                            future::ok(last_step)
                        }
                        // Not a step/id/cache, append all messages in between
                        Message::Stream { mut stream } => match last_step {
                            Transform::Id(msg) => future::ok(Transform::Id(msg)),
                            Transform::Empty => future::ok(Transform::Error(stream)),
                            Transform::Error(msg) => {
                                stream.push_str(&msg);
                                future::ok(Transform::Error(stream))
                            }
                        },
                        // compilation error. last step is supposed to have the compile error
                        Message::ErrorDetail { .. } => future::ok(last_step),
                    }
                })
                .and_then(|msg| match msg {
                    Transform::Error(msg) => future::err(ExecutionError::CompileError(msg)),
                    Transform::Empty => unreachable!(),
                    Transform::Id(id) => future::ok(id),
                })
                .and_then(move |id| {
                    trace!("building container from: {}", id);
                    let config = json!({
                        "NetworkDisabled": true,
                        "Image": id,
                        "HostConfig": {
                            "CpusetCpus": "2-3",
                            "PidsLimit": 1024,
                            "Ulimits": [{
                                "Name": "cpu",
                                "Hard": 1,
                                "Soft": 1
                             }],
                             "AutoRemove": true,
                             "Memory": 1073741824usize,
                             "MemorySwap": 1073741824usize,
                             "DiskQuota": 10737418240usize
                         }
                    });
                    ContainerBuilder::new()
                        .with_body(config.as_object().unwrap().clone())
                        .with_header(ContentType::json())
                        .build_on(&client2)
                        .map_err(|e| {
                            debug!("can't build container: {:?}", e);
                            ExecutionError::UnknownError
                        })
                        .map(|id| (client2, id))
                })
                .and_then(move |(client, id)| {
                    client
                        .start_container(&id)
                        .map_err(|e| {
                            debug!("cant start container: {:?}", e);
                            ExecutionError::UnknownError
                        })
                        .and_then(|_| Ok((client, id)))
                })
                .and_then(|(client, id)| {
                    trace!("getting logs from container: {}", id);
                    client
                        .logs(&id)
                        .map_err(|e| {
                            debug!("can't get logs: {:?}", e);
                            ExecutionError::UnknownError
                        })
                        .and_then(|logs| {
                            logs.map_err(|e| {
                                debug!("logging error: {:?}", e);
                                ExecutionError::UnknownError
                            }).fold(
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
                                .and_then(|(stdout, stderr)| {
                                    Ok(Output::Output {
                                        stdout: stdout,
                                        stderr: stderr,
                                    })
                                })
                        })
                })
                .then(|result| match result {
                    Ok(output) => future::ok(output),
                    Err(ExecutionError::CompileError(msg)) => {
                        future::ok(Output::CompileError { error: msg })
                    }
                    Err(e) => {
                        debug!("error in executor: {:?}", e);
                        future::err(e)
                    }
                })
        });
        Box::new(logs)
    }
}
