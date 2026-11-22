pub mod app;
#[rustfmt::skip]
#[allow(clippy::extra_unused_type_parameters)]
pub mod cereal;
pub mod common;
pub mod controllers;
pub mod framework;
pub mod initializers;
pub mod middleware;
pub mod models;
pub mod tasks;
pub mod views;
pub mod workers;

pub use framework as loco_rs;
