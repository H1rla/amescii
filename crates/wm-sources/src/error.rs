//! エラー型。

use thiserror::Error;

#[derive(Error, Debug)]
pub enum SourceError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON parse failed: {0}")]
    Json(#[from] serde_json::Error),

    #[error("image decode failed: {0}")]
    Image(#[from] image::ImageError),

    #[error("API returned no usable data for this location")]
    NoData,

    #[error("nowcast target time unavailable")]
    NoTargetTime,

    #[error("grid capacity exceeded for requested area")]
    GridTooLarge,

    #[error("missing API key for {0}")]
    MissingApiKey(&'static str),

    #[error("unexpected response shape: {0}")]
    Shape(String),
}

pub type Result<T> = std::result::Result<T, SourceError>;
