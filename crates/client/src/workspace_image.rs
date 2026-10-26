//! Shared bounded workspace-image transfer for desktop and mobile clients.
use base64::Engine as _;
use std::future::Future;
use zeron_proto::{
    MAX_WORKSPACE_IMAGE_BYTES, ReadWorkspaceImageRequest, WORKSPACE_IMAGE_CHUNK_BYTES,
    WorkspaceImageChunk,
};

#[derive(Debug, thiserror::Error)]
pub enum ImageReadError<E> {
    #[error("{0}")]
    Request(E),
    #[error("{0}")]
    Invalid(String),
}

pub async fn read_image<E, F, Fut>(
    mut request: ReadWorkspaceImageRequest,
    mut fetch: F,
) -> Result<(String, Vec<u8>), ImageReadError<E>>
where
    F: FnMut(ReadWorkspaceImageRequest) -> Fut,
    Fut: Future<Output = Result<WorkspaceImageChunk, E>>,
{
    if request.expected_checkout_id.is_empty() && !request.path.starts_with('/') {
        return Err(ImageReadError::Invalid(
            "Workspace checkout identity unavailable".into(),
        ));
    }
    let mut bytes = Vec::new();
    let mut mime = None;
    let mut size = None;
    for _ in 0..=MAX_WORKSPACE_IMAGE_BYTES / WORKSPACE_IMAGE_CHUNK_BYTES {
        let chunk: zeron_proto::WorkspaceImageChunk = fetch(request.clone())
            .await
            .map_err(ImageReadError::Request)?;
        if chunk.checkout_id != request.expected_checkout_id
            || chunk.content_hash.is_empty()
            || chunk.size > MAX_WORKSPACE_IMAGE_BYTES
            || chunk.data.len() > WORKSPACE_IMAGE_CHUNK_BYTES.div_ceil(3) * 4
            || request
                .expected_content_hash
                .as_ref()
                .is_some_and(|hash| hash != &chunk.content_hash)
            || mime.as_ref().is_some_and(|m| m != &chunk.mime_type)
            || size.is_some_and(|s| s != chunk.size)
        {
            return Err(ImageReadError::Invalid(
                "Image identity or size changed".into(),
            ));
        }
        let part = base64::engine::general_purpose::STANDARD
            .decode(&chunk.data)
            .map_err(|e| ImageReadError::Invalid(e.to_string()))?;
        if part.is_empty()
            || request.offset.checked_add(part.len()) != Some(chunk.next_offset)
            || chunk.next_offset > chunk.size
            || chunk.done != (chunk.next_offset == chunk.size)
        {
            return Err(ImageReadError::Invalid("Invalid image chunk offset".into()));
        }
        bytes.extend(part);
        if chunk.done {
            return Ok((chunk.mime_type, bytes));
        }
        request.offset = chunk.next_offset;
        request.expected_content_hash = Some(chunk.content_hash);
        mime = Some(chunk.mime_type);
        size = Some(chunk.size);
    }
    Err(ImageReadError::Invalid("Image chunk limit exceeded".into()))
}
