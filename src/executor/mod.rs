mod client;
mod container;
mod error;
mod image;
mod log;

use self::client::Docker;
use self::container::ContainerBuilder;
use self::error::DockerError;
use self::image::{ImageBuilder, Message};
use hyper::client::Connect;
use hyper::header::ContentType;
use hyper::server::Service;
use tokio_core::reactor::Handle;

use futures::{Future, Stream};

use tar::{Builder, Header};

use Language;
use Output;
use Submission;

use cpupool::CpuPool;
use futures::future;

use std::fs::File;
use std::path::Path;
use std::rc::Rc;

/// Builds a tar with files necessary for building a docker image for submission
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
    /// Should the filename where the code is to be saved
    fn get_file_name(&self) -> &'static str;
    /// Should return the docker file to be used for this container
    fn get_docker_file(&self) -> &'static str;
}

impl LanguageConfig for Language {
    fn get_file_name(&self) -> &'static str {
        match *self {
            Language::C => "code.c",
            Language::Python27 => "code.py",
        }
    }

    fn get_docker_file(&self) -> &'static str {
        match *self {
            Language::C => "resources/c/Dockerfile",
            Language::Python27 => "resources/python2/Dockerfile",
        }
    }
}

#[derive(Debug)]
pub enum ExecutionError {
    /// Error building a tar for build step
    BadConfig,
    /// Error communicating with the Docker client
    DockerError(DockerError),
    /// Holds the Compilation error message
    CompileError(String),
    UnknownError,
}

/// Executor implementation which uses the Docker backend
#[derive(Clone)]
pub struct Executor<C> {
    /// Singleton Docker client instance
    docker: Rc<Docker<C>>,
    /// Thread pool used for doing blocking operations
    pool: CpuPool,
}

impl<C: Connect> Executor<C> {
    /// Create a new Executor
    /// # Arguments
    /// * `connector` - Provides connection to where Docker is running
    /// * `handle` - A `Handle` to event loop on which this executor is to be run
    pub fn new(connector: C, handle: Handle) -> Self {
        Executor {
            docker: Rc::new(Docker::new(connector, handle)),
            pool: CpuPool::new(1),
        }
    }
}

/// Intermediate structure used when trying
/// to extract the image id out of build messages
enum Transform {
    /// The extracted id
    Id(String),
    /// Holds the message where an unexpected error happened
    Error(String),
    /// Initial State
    Empty,
}

impl<C: Connect> Service for Executor<C> {
    type Request = Submission;
    type Response = Output;
    type Error = ExecutionError;
    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    /// The steps that we do for a single execution are
    /// * Build a tar with the Dockerfile, code
    /// * Build an `Image` from the tar, this also compiles the code
    /// * Create a `Container` using the `Image` we build
    /// * Start the `Container` to run the program
    /// * Read the `Container` logs which contains the program output
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
            // This is a huge mess.
            // We are trying to extract the Id of the Docker Image we built.
            // The format of the docker response is not really suitable for
            // parsing and I barely managed to do so.
            //
            // We also compile the code when we build the Docker Image, so
            // compile errors are also extracted in that case. For interpreted
            // languages errors are extracted when the container is actually run
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
                                        // FIXME: Huge outputs may cause out of memory
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
