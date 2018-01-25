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
        }
    }

    fn get_docker_file(&self) -> &'static str {
        match *self {
            Language::C => "resources/c/Dockerfile",
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

#[cfg(test)]
mod tests {
    use super::Docker;
    use tokio_core::reactor::Core;
    use hyperlocal::UnixConnector;
    use docker::image::ImageBuilder;
    use docker::container::ContainerBuilder;
    use hyper::header::ContentType;
    use std::fs::{self, File};
    use futures::{self, Future, Stream};
    use std::path::Path;
    use tar::{Builder, Header};
    use json;
    use hyper;
    use docker::container::Containers;

    const DOCKERFILE: &str = "resources/Dockerfile";
    const HELLO_WORLD: &str = "resources/hello.c";

    //#[test]
    fn test_image_list() {
        let mut core = Core::new().unwrap();
        let docker = Docker::<UnixConnector>::new(core.handle());
        let images = docker.images();

        core.run(images).unwrap();
    }

    //#[test]
    fn test_container_list() {
        let mut core = Core::new().unwrap();
        let docker = Docker::<UnixConnector>::new(core.handle());
        let containers = docker.containers();
        core.run(containers).unwrap();
    }

    //#[test]
    fn test_image_build() {
        let mut core = Core::new().unwrap();
        let docker = Docker::<UnixConnector>::new(core.handle());
        let tar = make_tar();
        let builder = ImageBuilder::new().with_body(tar);
        let stream = docker.create_image(builder);
        let run = stream.and_then(|progress| {
            progress.for_each(|msg| {
                assert!(msg.is_ok());
                // if msg.is_ok() {
                //     println!("{}", msg.unwrap());
                // }
                Ok(())
            })
        });
        core.run(run).unwrap();
    }

    //#[test]
    fn test_image_build_quiet() {
        let mut core = Core::new().unwrap();
        let docker = Docker::<UnixConnector>::new(core.handle());
        let tar = make_tar();
        let builder = ImageBuilder::new().with_body(tar).with_param("q", "true");
        let stream = docker.create_image_quiet(builder);
        let run = stream
            .and_then(|progress| {
                assert!(progress.get("stream").is_some());
                Ok(())
            })
            .or_else(|err| {
                println!("{:?}", err);
                futures::future::err(())
            });
        core.run(run).unwrap();
    }

    //#[test]
    fn test_chain() {
        let mut core = Core::new().unwrap();
        let docker = Docker::<UnixConnector>::new(core.handle());
        let tar = make_tar();
        let builder = ImageBuilder::new().with_body(tar).with_param("q", "true");
        let stream = docker.create_image(builder);
        let run = stream
            .and_then(|progress| {
                progress.for_each(|msg| {
                    let js = json::to_string(&msg.unwrap()).unwrap();
                    let id: json::Map<String, json::Value> = json::from_str(&js).unwrap();
                    futures::future::ok(())
                })
            })
            .or_else(|err| futures::future::err(()));
        core.run(run).unwrap();
    }

    //#[test]
    fn test_container_build() {
        // delete container before running test
        let mut core = Core::new().unwrap();
        let docker = Docker::<UnixConnector>::new(core.handle());
        let map: json::Map<String, json::Value> =
            json::from_str(r#"{"Image": "hello-world:latest", "Command": ["/hello"]}"#).unwrap();
        let mut builder = ContainerBuilder::new().with_body(map);
        builder.set_param("name", "mycontainer");
        builder.set_header(ContentType::json());
        let container = docker.create_container(builder);
        let map = core.run(container).unwrap();
        assert!(map.get("Id").is_some());
    }

    fn make_tar() -> Vec<u8> {
        let mut builder = Builder::new(Vec::new());
        let mut dockerfile = File::open(DOCKERFILE).unwrap();
        let mut hello_world = File::open(HELLO_WORLD).unwrap();
        builder
            .append_file(Path::new("Dockerfile"), &mut dockerfile)
            .unwrap();
        builder
            .append_file(Path::new("hello.c"), &mut hello_world)
            .unwrap();
        builder.into_inner().unwrap()
    }

    //#[test]
    fn test_images_chain() {
        use docker::Images;
        let mut core = Core::new().unwrap();
        let docker = Docker::<UnixConnector>::new(core.handle());
        let tar = make_tar();
        let builder = ImageBuilder::new().with_body(tar).with_param("q", "true");
        let images = Images::create_image_with(docker, builder);
        let chain = images
            .and_then(|(docker, progress)| {
                progress
                    .for_each(|msg| {
                        let js = json::to_string(&msg.unwrap()).unwrap();
                        let id: json::Map<String, json::Value> = json::from_str(&js).unwrap();
                        futures::future::ok(())
                    })
                    .map_err(|e| (docker, e))
            })
            .or_else(|(_, err)| futures::future::err(err));
        core.run(chain).unwrap();
    }

    //#[test]
    //#fn test_container_start() {
    //#let mut core = Core::new().unwrap();
    //#let docker = Docker::<UnixConnector>::new(core.handle());
    //#let map: json::Map<String, json::Value> = json::from_str(
    //#		r#"{"Image": "hello-world:latest", "Command": ["/hello"]}"#
    //#	   ).unwrap();
    //#let mut builder = ContainerBuilder::new().with_body(map);
    //#builder.set_param("name", "mycontainer");
    //#builder.set_header(ContentType::json());
    //#let container = Containers::create_container_with(docker, builder)
    //#		.and_then(|(docker, container)| {
    //#let id = container.get("Id").unwrap();
    //#docker.start_container(id).map(|_| id)
    //#});
    //
    //#let id = core.run(container).unwrap();
    //
    //#assert!(id.get("Id").is_some());
    //#}

    #[test]
    fn test_container_logs() {
        let mut core = Core::new().unwrap();
        let docker = Docker::<UnixConnector>::new(core.handle());
        let logs = docker.logs("3901a37be11c").and_then(move |stream| {
            stream
                .map_err(|_| hyper::Error::Incomplete)
                .for_each(|message| {
                    println!("{}", message);
                    futures::future::ok(())
                })
        });
        let logs = core.run(logs).unwrap();
    }
}
