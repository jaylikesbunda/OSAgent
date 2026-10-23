pub mod api;
pub mod browser;
pub mod files;
pub mod github;
pub mod remote;
pub mod scheduler;
pub mod ws;

pub use api::create_router;
