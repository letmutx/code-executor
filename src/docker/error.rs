use hyper::Error;
use docker::image::BuilderError;

pub enum DockerError {
    HyperError(Error),
    ImageBuilderError(BuilderError),
}
