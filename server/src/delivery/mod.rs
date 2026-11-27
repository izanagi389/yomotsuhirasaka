//! 配信API（GraphQL）。02-schema-and-delivery-api.md「配信API」、13-security.md「GraphQL」。

pub mod ai;
mod ai_groups;
pub mod answer;
pub mod data;
pub mod limiter;
mod list_filter;
pub mod names;
pub mod queries;
pub mod schema;
mod template_fields;
pub mod validate;
