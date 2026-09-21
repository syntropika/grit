use url::Url;

use crate::{github::RepositoryMetadata, graph::GraphError};

pub(crate) struct ConfirmedPublicRepository {
    pub(super) full_name: String,
    pub(super) web_url: Url,
}

pub(crate) fn confirm_public_repository(
    expected_repository: &str,
    metadata: RepositoryMetadata,
) -> Result<ConfirmedPublicRepository, GraphError> {
    if metadata.private || !metadata.visibility.eq_ignore_ascii_case("public") {
        return Err(GraphError::RepositoryNotConfirmedPublic {
            visibility: metadata.visibility,
            private: metadata.private,
        });
    }
    if !metadata.full_name.eq_ignore_ascii_case(expected_repository) {
        return Err(GraphError::PublicRepositoryMismatch {
            expected: expected_repository.to_owned(),
            actual: metadata.full_name,
        });
    }
    let mut web_url =
        Url::parse(&metadata.html_url).map_err(|_| GraphError::InvalidPublicRepositoryUrl)?;
    if web_url.scheme() != "https"
        || web_url.host_str().is_none()
        || !web_url.username().is_empty()
        || web_url.password().is_some()
        || web_url.query().is_some()
        || web_url.fragment().is_some()
        || !web_url
            .path()
            .trim_end_matches('/')
            .eq_ignore_ascii_case(&format!("/{expected_repository}"))
    {
        return Err(GraphError::InvalidPublicRepositoryUrl);
    }
    let normalized_path = web_url.path().trim_end_matches('/').to_owned();
    web_url.set_path(&normalized_path);
    Ok(ConfirmedPublicRepository {
        full_name: metadata.full_name,
        web_url,
    })
}

impl ConfirmedPublicRepository {
    pub(super) fn issue_url(&self, number: u64) -> String {
        format!("{}/issues/{number}", self.web_url)
    }
}
