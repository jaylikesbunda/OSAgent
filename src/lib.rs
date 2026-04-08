#![allow(dead_code)]
#![allow(unused_imports)]
#![allow(unused_variables)]
#![allow(unused_mut)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]
#![allow(clippy::ptr_arg)]
#![allow(clippy::manual_strip)]
#![allow(clippy::empty_line_after_doc_comments)]
#![allow(clippy::should_implement_trait)]
#![allow(clippy::map_identity)]
#![allow(clippy::vec_init_then_push)]
#![allow(clippy::match_like_matches_macro)]

pub mod agent;
pub mod config;
#[cfg(feature = "discord")]
pub mod discord;
pub mod error;
pub mod external;
pub mod indexer;
pub mod lsp;
pub mod oauth;
pub mod permission;
pub mod plugin;
pub mod prompt_eval;
pub mod scheduler;
pub mod skills;
pub mod storage;
pub mod tools;
pub mod update;
pub mod voice;
pub mod web;
pub mod workflow;
