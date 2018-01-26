extern crate bytes;
extern crate env_logger;
extern crate futures;
extern crate futures_cpupool as cpupool;
extern crate hyper;
extern crate hyperlocal;
#[macro_use]
extern crate log as logger;
extern crate serde;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json as json;
extern crate tar;
extern crate tokio_core;
extern crate unicase;
extern crate url;

mod executor;

use hyper::server::Http;
use hyper::server::Service;
use hyper::server::Response;
use hyperlocal::UnixConnector;
use hyper::{Body, Method, StatusCode};

use futures::{future, Future};
use futures::Stream;

use std::rc::Rc;
use std::clone::Clone;

use tokio_core::reactor::Core;

use executor::Executor;
use executor::ExecutionError;

#[derive(Serialize, Deserialize, Debug)]
pub struct Submission {
    code: String,
    lang: Language,
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone)]
enum Language {
    #[serde(rename = "c")]
    C,
    #[serde(rename = "python2.7")]
    Python27,
}

type Stdout = String;
type Stderr = String;

#[derive(Serialize)]
pub enum Output {
    #[serde(rename = "compile_error")] CompileError { error: String },
    #[serde(rename = "output")] Output { stdout: Stdout, stderr: Stderr },
}

#[derive(Clone)]
struct APIService<E> {
    executor: Rc<E>,
}

impl<E> APIService<E> {
    fn new(executor: E) -> Self {
        APIService { executor: Rc::new(executor) }
    }
}

#[derive(Debug)]
enum APIError {
    BadRequest,
    HyperError,
    ExecutionError,
}

impl<E> Service for APIService<E>
where
    E: Service<Request = Submission, Response = Output, Error = ExecutionError> + 'static,
{
    type Request = hyper::server::Request;
    type Response = hyper::server::Response;
    type Error = hyper::Error;
    type Future = Box<Future<Item = Self::Response, Error = Self::Error>>;

    fn call(&self, req: Self::Request) -> Self::Future {
        match (req.method(), req.path()) {
            (&Method::Post, "/execute") => {
                trace!("execute request");
                let executor = self.executor.clone();
                let response = req.body()
                    .fold(Vec::new(), |mut body, chunk| {
                        body.extend(chunk.into_iter());
                        future::ok::<_, hyper::Error>(body)
                    })
                    .map_err(|e| {
                        debug!("can't read body: {:?}", e);
                        APIError::HyperError
                    })
                    .and_then(|json| match json::from_slice::<Submission>(&json) {
                        Ok(sub) => future::ok(sub),
                        _ => future::err(APIError::BadRequest),
                    })
                    .and_then(move |sub: Submission| {
                        executor
                            .call(sub)
                            .map_err(|e| {
                                debug!("executor error: {:?}", e);
                                APIError::ExecutionError
                            })
                            .and_then(|resp| {
                                future::ok(
                                    Response::new()
                                        .with_body(Body::from(json::to_string(&resp).expect("can't error"))),
                                )
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
                        future::ok(response)
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
    env_logger::init();
    let mut core = Core::new().unwrap();
    let handle = &core.handle();
    let addr = "127.0.0.1:3000".parse().unwrap();
    let executor = Executor::new(UnixConnector::new(handle.clone()), handle.clone());
    let api_service = APIService::new(executor);
    let handle2 = handle.clone();
    let server = Http::new().serve_addr_handle(&addr, handle, || Ok(api_service.clone()))
            .expect("can't start serve")
            .for_each(move |conn| {
                let handle = &handle2;
                handle.spawn(conn.map(|_| ()).map_err(|e| {
                    debug!("conn error: {:?}", e);
                }));
                Ok(())
            }).map_err(|e| debug!("error: {:?}", e));
    core.run(server).unwrap();
}
