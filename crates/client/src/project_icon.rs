//! Project artwork fetched from its owning host; never persisted as a local file.
use crate::error::Result;
use crate::{Client, ClientError};
use zeron_proto::WorkspaceTarget;

impl Client {
    /// Repository artwork candidates for the native project editor.
    pub async fn project_images(&self, space_id: &str, query: &str) -> Result<Vec<String>> {
        let (device_id, target) = self.project_artwork_target(space_id)?;
        let request = zeron_proto::SearchWorkspaceFilesRequest {
            target,
            query: zeron_proto::project_icon::browse_query(query),
            include_ignored: false,
            limit: Some(200),
        };
        let value = self
            .inner
            .host_rpc(
                &device_id,
                crate::rpc::methods::SEARCH_WORKSPACE_FILES,
                serde_json::to_value(request)
                    .map_err(|_| ClientError::InvalidArgument("Invalid image search".into()))?,
            )
            .await?;
        let matches: Vec<zeron_proto::WorkspaceFileSearchMatch> = serde_json::from_value(value)
            .map_err(|_| ClientError::Internal("Invalid image search response".into()))?;
        Ok(matches
            .into_iter()
            .filter(|file| file.kind == zeron_proto::WorkspaceEntryKind::File)
            .map(|file| file.path)
            .collect())
    }

    fn project_artwork_target(&self, space_id: &str) -> Result<(String, WorkspaceTarget)> {
        let state = self.inner.workspace.state().0;
        let space = state
            .spaces
            .iter()
            .find(|space| space.id == space_id)
            .ok_or_else(|| ClientError::NotFound("Project".into()))?;
        Ok((
            space.device_id.clone(),
            WorkspaceTarget {
                chat_id: None,
                space_id: Some(space.id.clone()),
                checkout_path: None,
            },
        ))
    }

    /// Preview a selection without saving it. `None` resolves automatic artwork.
    pub async fn project_image(
        &self,
        space_id: &str,
        path: Option<&str>,
    ) -> Result<(String, Vec<u8>)> {
        let icon_path = zeron_proto::project_icon::normalize_path(path)
            .map_err(|error| ClientError::InvalidArgument(error.into()))?;
        let (device_id, target) = self.project_artwork_target(space_id)?;
        for path in zeron_proto::project_icon::candidates(icon_path.as_deref()) {
            let response = self
                .inner
                .host_rpc(
                    &device_id,
                    crate::rpc::methods::READ_WORKSPACE_FILE,
                    serde_json::to_value(zeron_proto::ReadWorkspaceFileRequest {
                        target: target.clone(),
                        path: path.into(),
                    })
                    .map_err(|_| ClientError::InvalidArgument("Invalid image request".into()))?,
                )
                .await;
            let file: zeron_proto::WorkspaceFileText = match response {
                Ok(value) => serde_json::from_value(value)
                    .map_err(|_| ClientError::Internal("Invalid workspace response".into()))?,
                Err(ClientError::HostUnavailable(message)) => {
                    return Err(ClientError::HostUnavailable(message));
                }
                Err(_) if icon_path.is_none() => continue,
                Err(error) => return Err(error),
            };
            return self
                .project_icon_image(&device_id, target, path.into(), file.checkout_id)
                .await;
        }
        Err(ClientError::NotFound("Project artwork".into()))
    }
    /// The project's saved selection, resolved through its owning host.
    pub async fn project_icon(&self, space_id: &str) -> Result<(String, Vec<u8>)> {
        let icon_path = {
            let state = self.inner.workspace.state().0;
            let space = state
                .spaces
                .iter()
                .find(|space| space.id == space_id)
                .ok_or_else(|| ClientError::NotFound("Project".into()))?;
            zeron_proto::view::representative_space(&state.spaces, space)
                .icon_path
                .clone()
        };
        self.project_image(space_id, icon_path.as_deref()).await
    }

    async fn project_icon_image(
        &self,
        device_id: &str,
        target: WorkspaceTarget,
        path: String,
        checkout_id: String,
    ) -> Result<(String, Vec<u8>)> {
        let request = zeron_proto::ReadWorkspaceImageRequest {
            target,
            path,
            expected_checkout_id: checkout_id,
            offset: 0,
            expected_content_hash: None,
        };
        crate::workspace_image::read_image(request, |request| async move {
            let value = self
                .inner
                .host_rpc(
                    device_id,
                    crate::rpc::methods::READ_WORKSPACE_IMAGE,
                    serde_json::to_value(request).map_err(|_| {
                        ClientError::InvalidArgument("Invalid image request".into())
                    })?,
                )
                .await?;
            serde_json::from_value(value)
                .map_err(|_| ClientError::Internal("Invalid image response".into()))
        })
        .await
        .map_err(|error| match error {
            crate::workspace_image::ImageReadError::Request(error) => error,
            crate::workspace_image::ImageReadError::Invalid(message) => {
                ClientError::Internal(message)
            }
        })
    }
}
