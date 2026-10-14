//! rvw's domain: types, operations, the storage trait and the service.
pub mod error;
mod evidence;
mod gate;
pub mod operations;
pub mod repository;
pub mod service;
pub mod types;
mod verdict;

pub use error::{Error, ErrorKind, Result};
pub use gate::undecided;
pub use repository::{Repository, Tx};
pub use service::Service;
