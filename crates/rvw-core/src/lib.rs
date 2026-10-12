//! rvw's domain: types, operations, the storage trait and the service.
pub mod error;
mod evidence;
pub mod operations;
pub mod repository;
pub mod service;
pub mod types;

pub use error::{Error, ErrorKind, Result};
pub use repository::{Repository, Tx};
pub use service::Service;
