//! The service: every review operation against a [`Repository`].
//! Slice 1 fills this in (port of internal/review/service.go and evidence.go).

use crate::repository::Repository;

pub struct Service {
    #[allow(dead_code)]
    repo: Box<dyn Repository>,
}

impl Service {
    pub fn new(repo: Box<dyn Repository>) -> Self {
        Self { repo }
    }
}
