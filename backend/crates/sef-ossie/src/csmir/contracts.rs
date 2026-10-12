//! Contracts that span objects: as-of relationships, bridges with
//! allocation and row policies, plus the references they share (a
//! `dataset.field`, a grain atom, an ordering).
//!
//! All are declared in our extension because Ossie 0.2 has no place for
//! them; each is checked against the model it refers to, so a typo is an
//! error, never an ignored contract.

use sef_model_ir::expressions::{ScalarExpr, ScalarExprKind};
use sef_model_ir::grain::GrainAtom;
use sef_model_ir::identity::StableId;
use sef_model_ir::policy::{ContextSlot, Enforcement, Policy, PolicyScope, Sensitivity};
use sef_model_ir::relationships::{
    Allocation, Bridge, ContractEvidenceState, IntervalBoundary, MatchCardinality,
    TemporalConstraint,
};
use sef_model_ir::time::{IanaZoneId, NullPlacement, OrderTerm, SortDirection};
use sef_model_ir::types::{DecimalValue, ValueType};

use super::scalar::{Failure, Names, Translator, describe, not_representable};
use super::{Converter, RowScope};
use crate::diagnostic::Code;
use crate::extension::{
    AllocationDeclaration, BoundaryDeclaration, BridgeDeclaration, EnforcementDeclaration,
    MatchDeclaration, OrderDeclaration, PolicyDeclaration, SensitivityDeclaration, SlotType,
    TemporalDeclaration,
};
use crate::ids;
use crate::validate::normalize_identifier;

impl Converter<'_> {
    /// The column field `dataset.field` names.
    pub(super) fn qualified_field(&self, reference: &str) -> Result<StableId, Failure> {
        let unknown = || {
            Failure::new(
                Code::UnknownReference,
                format!("{reference:?} no es un campo de columna (escríbelo como dataset.campo)."),
            )
        };
        let (dataset, field) = reference.split_once('.').ok_or_else(unknown)?;
        let d = self.dataset_index(dataset).ok_or_else(unknown)?;
        self.column_field(d, &normalize_identifier(field))
            .map(|(id, _)| id)
            .ok_or_else(unknown)
    }

    /// An ordering over a column field or a dimension.
    pub(super) fn order_term(&self, declared: &OrderDeclaration) -> Result<OrderTerm, Failure> {
        let value = match (&declared.field, &declared.dimension) {
            (Some(field), None) => self.qualified_field(field)?,
            (None, Some(name)) => match self.atom(Some(name), None)? {
                GrainAtom::DimensionRef { dimension, .. } => dimension,
                _ => return Err(not_representable("Ese orden")),
            },
            _ => {
                return Err(Failure::new(
                    Code::InvalidExtension,
                    "Cada orden nombra exactamente un campo o una dimensión.",
                ));
            }
        };
        Ok(OrderTerm {
            value,
            direction: if declared.descending {
                SortDirection::Descending
            } else {
                SortDirection::Ascending
            },
            nulls: if declared.nulls_first {
                NullPlacement::First
            } else {
                NullPlacement::Last
            },
        })
    }

    /// An entity key by its name (`account_snapshot`).
    pub(super) fn entity_key(&self, name: &str) -> Result<StableId, Failure> {
        let segment = ids::segment(name);
        self.entities
            .values()
            .flatten()
            .map(|key| key.metadata.id())
            .find(|id| id.as_str().rsplit('/').next() == Some(segment.as_str()))
            .cloned()
            .ok_or_else(|| {
                Failure::new(
                    Code::UnknownReference,
                    format!("No existe la clave {name:?}."),
                )
            })
    }

    pub(super) fn temporal(
        &self,
        declared: &TemporalDeclaration,
    ) -> Result<TemporalConstraint, Failure> {
        Ok(TemporalConstraint {
            event_time: self.qualified_field(&declared.event_time)?,
            valid_from: self.qualified_field(&declared.valid_from)?,
            valid_to: declared
                .valid_to
                .as_deref()
                .map(|field| self.qualified_field(field))
                .transpose()?,
            interval_boundary: match declared.boundary {
                BoundaryDeclaration::ClosedOpen => IntervalBoundary::ClosedOpen,
                BoundaryDeclaration::ClosedClosed => IntervalBoundary::ClosedClosed,
                BoundaryDeclaration::OpenClosed => IntervalBoundary::OpenClosed,
                BoundaryDeclaration::OpenOpen => IntervalBoundary::OpenOpen,
            },
            match_cardinality: match declared.match_cardinality {
                MatchDeclaration::ExactlyOne => MatchCardinality::ExactlyOne,
                MatchDeclaration::AtMostOne => MatchCardinality::AtMostOne,
            },
            evidence: ContractEvidenceState::DeclaredUnverified,
        })
    }

    /// Bridges of the model extension.
    pub(super) fn bridges(&mut self) {
        let declared = self.extension.bridges.clone();
        for (b, bridge) in declared.iter().enumerate() {
            let path = format!("/custom_extensions/bridges/{b}");
            match self.bridge(bridge, &path) {
                Ok(bridge) => self.bridges.push(bridge),
                Err(failure) => self.fail(&path, failure),
            }
        }
    }

    fn bridge(&mut self, declared: &BridgeDeclaration, path: &str) -> Result<Bridge, Failure> {
        let dataset = |name: &str| {
            self.dataset_index(name).ok_or_else(|| {
                Failure::new(
                    Code::UnknownReference,
                    format!("No existe el dataset {name:?}."),
                )
            })
        };
        let (left, middle, right) = (
            dataset(&declared.left)?,
            dataset(&declared.bridge)?,
            dataset(&declared.right)?,
        );
        let relationship = |name: &str, to: usize| {
            let segment = ids::segment(name);
            self.relationships
                .iter()
                .find(|relationship| relationship.role == segment)
                .filter(|relationship| {
                    relationship.from_dataset == self.datasets[middle].id
                        && relationship.to_dataset == self.datasets[to].id
                })
                .map(|relationship| relationship.metadata.id().clone())
                .ok_or_else(|| {
                    Failure::new(
                        Code::UnknownReference,
                        format!(
                            "La relación {name:?} debe ir del puente {:?} a {:?}.",
                            declared.bridge, self.model.datasets[to].name
                        ),
                    )
                })
        };
        let left_relationship = relationship(&declared.left_relationship, left)?;
        let right_relationship = relationship(&declared.right_relationship, right)?;
        let allocation = declared
            .allocation
            .as_ref()
            .map(|allocation| self.allocation(middle, allocation, path))
            .transpose()?;
        let metadata = self
            .metadata("bridge", &ids::segment(&declared.name), path)
            .ok_or_else(|| Failure::new(Code::InvalidIdentifier, "Puente sin identificador."))?;
        Ok(Bridge {
            metadata,
            left_dataset: self.datasets[left].id.clone(),
            bridge_dataset: self.datasets[middle].id.clone(),
            right_dataset: self.datasets[right].id.clone(),
            left_relationship,
            right_relationship,
            allocation,
        })
    }

    fn allocation(
        &mut self,
        bridge: usize,
        declared: &AllocationDeclaration,
        path: &str,
    ) -> Result<Allocation, Failure> {
        let ast = crate::expression::parse(&declared.weight)
            .map_err(|diagnostic| Failure::new(diagnostic.code, diagnostic.message))?;
        let weight = self.row_expression(bridge, &ast, None, super::scalar::Division::default())?;
        let ValueType::Decimal(decimal) = weight.value_type else {
            return Err(Failure::new(
                Code::TypeMismatch,
                format!(
                    "El peso de una asignación es un DECIMAL exacto y aquí es {}.",
                    describe(&weight.value_type)
                ),
            ));
        };
        let exact = |text: &str| {
            let (integer, fraction) = text.split_once('.').unwrap_or((text, ""));
            let scale = usize::from(decimal.scale());
            if fraction.len() > scale {
                return Err(Failure::new(
                    Code::TypeMismatch,
                    format!("{text:?} tiene más decimales que el peso ({scale})."),
                ));
            }
            DecimalValue::new(format!("{integer}{fraction:0<scale$}"), decimal)
                .map_err(|diagnostic| Failure::new(Code::TypeMismatch, diagnostic.message))
        };
        let normalization_scope =
            self.grain(&declared.normalization_scope, path)
                .ok_or_else(|| {
                    Failure::new(Code::UnknownReference, "Ámbito de normalización no válido.")
                })?;
        Ok(Allocation {
            weight_expression: weight,
            normalization_scope,
            required_total: exact(&declared.required_total)?,
            tolerance: exact(&declared.tolerance)?,
            evidence: ContractEvidenceState::DeclaredUnverified,
        })
    }

    /// Policies of the model extension (dataset scope).
    pub(super) fn policies(&mut self) {
        let declared = self.extension.policies.clone();
        for (p, policy) in declared.iter().enumerate() {
            let path = format!("/custom_extensions/policies/{p}");
            match self.policy(policy, &path) {
                Ok(policy) => self.policies.push(policy),
                Err(failure) => self.fail(&path, failure),
            }
        }
    }

    fn policy(&mut self, declared: &PolicyDeclaration, path: &str) -> Result<Policy, Failure> {
        let d = match (&declared.dataset, &declared.metric) {
            (Some(dataset), None) => self.dataset_index(dataset).ok_or_else(|| {
                Failure::new(
                    Code::UnknownReference,
                    format!("No existe el dataset {dataset:?}."),
                )
            })?,
            (None, Some(_)) => {
                return Err(not_representable(
                    "Una política sobre una métrica (de momento, solo sobre datasets)",
                ));
            }
            _ => {
                return Err(Failure::new(
                    Code::InvalidExtension,
                    "Una política se aplica a un dataset o a una métrica, no a ambos.",
                ));
            }
        };
        let slots: Vec<ContextSlot> = declared
            .context
            .iter()
            .map(|slot| ContextSlot {
                name: slot.name.clone(),
                value_type: match slot.value_type {
                    SlotType::String => ValueType::Utf8,
                    SlotType::Integer => ValueType::Integer {
                        signed: true,
                        bits: Some(64),
                    },
                    SlotType::Boolean => ValueType::Boolean,
                    SlotType::Date => ValueType::Date,
                },
                required: slot.required,
                sensitivity: match slot.sensitivity {
                    SensitivityDeclaration::Public => Sensitivity::Public,
                    SensitivityDeclaration::Sensitive => Sensitivity::Sensitive,
                    SensitivityDeclaration::Secret => Sensitivity::Secret,
                },
            })
            .collect();
        let ast = crate::expression::parse(&declared.predicate)
            .map_err(|diagnostic| Failure::new(diagnostic.code, diagnostic.message))?;
        let mut scope = PredicateScope {
            row: RowScope {
                converter: self,
                dataset: d,
                timezone: None,
            },
            slots: &slots,
            used: Vec::new(),
        };
        let predicate = Translator {
            names: &mut scope,
            division: super::scalar::Division::default(),
        }
        .translate(&ast)?;
        let used = scope.used;
        if predicate.value_type != ValueType::Boolean {
            return Err(Failure::new(
                Code::TypeMismatch,
                "El predicado de una política es una condición (verdadero o falso).",
            ));
        }
        if let Some(unused) = slots.iter().find(|slot| !used.contains(&slot.name)) {
            return Err(Failure::new(
                Code::InvalidExtension,
                format!(
                    "El valor de contexto {:?} se declara y no se usa.",
                    unused.name
                ),
            ));
        }
        let metadata = self
            .metadata("policy", &ids::segment(&declared.name), path)
            .ok_or_else(|| Failure::new(Code::InvalidIdentifier, "Política sin identificador."))?;
        Ok(Policy {
            metadata,
            scope: PolicyScope::Dataset(self.datasets[d].id.clone()),
            enforcement: match declared.enforcement {
                EnforcementDeclaration::Mandatory => Enforcement::Mandatory,
                EnforcementDeclaration::OptionalGoverned => Enforcement::OptionalGoverned,
            },
            predicate,
            context_slots: slots,
        })
    }
}

/// A policy predicate: the dataset's columns plus declared context values.
struct PredicateScope<'s, 'c, 'a> {
    row: RowScope<'c, 'a>,
    slots: &'s [ContextSlot],
    used: Vec<String>,
}

impl Names for PredicateScope<'_, '_, '_> {
    fn reference(&mut self, qualifier: Option<&str>, name: &str) -> Result<ScalarExpr, Failure> {
        self.row.reference(qualifier, name)
    }

    fn timezone(&self) -> Option<&IanaZoneId> {
        None
    }

    fn context(&mut self, name: &str) -> Result<ScalarExpr, Failure> {
        let slot = self
            .slots
            .iter()
            .find(|slot| slot.name == name)
            .ok_or_else(|| {
                Failure::new(
                    Code::UnknownReference,
                    format!("`:{name}` no está declarado en \"context\" de la política."),
                )
            })?;
        if !self.used.contains(&slot.name) {
            self.used.push(slot.name.clone());
        }
        Ok(ScalarExpr {
            kind: ScalarExprKind::ContextSlot(slot.name.clone()),
            value_type: slot.value_type.clone(),
            unit: None,
        })
    }
}
