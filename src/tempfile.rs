/// AI generated tempfile extractor for Axum pleas fix

use axum::{
    async_trait,
    extract::{FromRequest, Request},
    http::StatusCode,
};
use tokio::{fs::File, io::AsyncWriteExt};
use tempfile::NamedTempFile;
use futures::StreamExt;

pub struct TempFile {
    pub path: std::path::PathBuf,
    pub file: NamedTempFile,
}

#[async_trait]
impl<S> FromRequest<S> for TempFile
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, String);

    async fn from_request(req: Request, _state: &S) -> Result<Self, Self::Rejection> {
        let mut body = req.into_body().into_data_stream();

        // Create a temp file
        let tmp = NamedTempFile::new()
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        let path = tmp.path().to_path_buf();
        let mut file = File::from_std(tmp.into_file());

        // Stream chunks to file
        while let Some(chunk_res) = body.next().await {
            let chunk = chunk_res.map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
            file.write_all(&chunk)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }

        Ok(TempFile { path })
    }
}