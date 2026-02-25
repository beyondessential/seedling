use std::sync::{Arc, Mutex};

type Holder<T> = Arc<Mutex<T>>;

pub mod action;
pub mod app;
pub mod deployment;
pub mod ingress;
pub mod install;
pub mod resource;
pub mod service;
