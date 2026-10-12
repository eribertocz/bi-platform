//! Descriptive governance and name-to-stable-ID conversion, outside canonical data.
use super::scalar::Failure;
use crate::diagnostic::Code;
use crate::extension::{ClassificationDeclaration, StatusDeclaration};
use crate::ids::{self, Ids};
use sef_model_ir::model::{Classification, Governance, Lifecycle};

fn convert(
    ids: &Ids,
    kind: &str,
    dataset: Option<&str>,
    owner: Option<&str>,
    tags: &[String],
    status: Option<&StatusDeclaration>,
    sensitivity: ClassificationDeclaration,
) -> Result<Governance, Failure> {
    let lifecycle = match status {
        None | Some(StatusDeclaration::Active) => Lifecycle::Active,
        Some(StatusDeclaration::Deprecated {
            sunset,
            replacement,
        }) => {
            let replacement = replacement
                .as_ref()
                .map(|name| {
                    // Unqualified fields resolve within their dataset. Default dimensions
                    // use dataset.field; conformed dimensions use their declared name.
                    let segment = match (kind, name.split_once('.'), dataset) {
                        ("field" | "dimension", Some((dataset, field)), _) => {
                            format!("{}.{}", ids::segment(dataset), ids::segment(field))
                        }
                        ("field" | "dimension", None, Some(dataset)) => {
                            format!("{dataset}.{}", ids::segment(name))
                        }
                        _ => ids::segment(name),
                    };
                    ids.id(kind, &segment).map_err(|_| {
                        Failure::new(
                            Code::InvalidIdentifier,
                            "Reemplazo sin identificador estable.",
                        )
                    })
                })
                .transpose()?;
            Lifecycle::Deprecated {
                sunset: sunset.clone(),
                replacement,
            }
        }
    };
    Ok(Governance {
        owner: owner.map(str::to_owned),
        tags: tags.to_vec(),
        lifecycle,
        classification: match sensitivity {
            ClassificationDeclaration::Public => Classification::Public,
            ClassificationDeclaration::Internal => Classification::Internal,
            ClassificationDeclaration::Confidential => Classification::Confidential,
            ClassificationDeclaration::Restricted => Classification::Restricted,
        },
    })
}

/// Implemented explicitly at each supported extension level, without flattening.
pub(super) trait Governed {
    fn governance(
        &self,
        ids: &Ids,
        kind: &str,
        dataset: Option<&str>,
    ) -> Result<Governance, Failure>;
}

macro_rules! governed {
    ($($ty:ty),+) => {$(
        impl Governed for $ty {
            fn governance(&self, ids: &Ids, kind: &str, dataset: Option<&str>) -> Result<Governance, Failure> {
                convert(ids, kind, dataset, self.owner.as_deref(), &self.tags, self.status.as_ref(), self.sensitivity)
            }
        }
    )+};
}
governed!(
    crate::extension::DatasetExtension,
    crate::extension::FieldExtension,
    crate::extension::MetricExtension
);
