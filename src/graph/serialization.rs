use serde::Serialize;

use super::GraphError;

pub(super) fn pretty_json<T: Serialize>(value: &T) -> Result<Vec<u8>, GraphError> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(GraphError::EncodeArtifact)?;
    bytes.push(b'\n');
    Ok(bytes)
}
