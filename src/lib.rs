pub mod app;
pub mod git;

pub type AppResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
