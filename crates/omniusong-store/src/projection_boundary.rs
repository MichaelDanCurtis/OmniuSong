//! A parsed ownership envelope is not authenticated performance derivation.
//! Remove this staged admission fence only when all publication/archive paths
//! recompute their retained reviews against the exact owned original.
use crate::{Result, StoreError};
use omniusong_core::ProjectDocument;

pub(crate) fn reject_unverified_projection(document: &ProjectDocument) -> Result<()> {
    if document
        .performance_projections()?
        .is_some_and(|registry| !registry.entries.is_empty())
    {
        return Err(StoreError::Invalid(
            "Performance projection ownership requires authenticated publication and archive validation on this path".into(),
        ));
    }
    Ok(())
}

/// A caller-built candidate cannot bypass the explicit promotion API and
/// overwrite a same-named field owned by an older document's author.
pub(crate) fn reject_opaque_promotion(
    before: &ProjectDocument,
    after: &ProjectDocument,
) -> Result<()> {
    if before.schema_version() < 3
        && after.schema_version() == 3
        && before.value().get("performanceProjections").is_some()
    {
        return Err(StoreError::Invalid(
            "Projection schema promotion would overwrite opaque performanceProjections metadata"
                .into(),
        ));
    }
    Ok(())
}
