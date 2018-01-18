use hyper;

// TODO expand unknown errors to other errors
#[derive(Debug)]
pub enum DockerError {
    HyperError(hyper::Error),
    BadRequest,
    InternalServerError,
    CantAttach,
    UnknownError,
    NotFound,
}
