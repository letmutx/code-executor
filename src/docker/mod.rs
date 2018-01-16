mod client;
pub mod log;
mod image;
mod error;
mod container;

pub use docker::image::{Detail, ImageBuilder, Message};
pub use docker::client::Docker;
pub use docker::log::Logs;
pub use docker::container::ContainerBuilder;
pub use docker::error::DockerError;

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
