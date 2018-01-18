use hyper;

#[derive(Debug)]
pub enum DockerError {
    HyperError(hyper::Error),
    BadRequest,
    InternalServerError,
    CantAttach,
    UnknownError,
    NotFound,
}
