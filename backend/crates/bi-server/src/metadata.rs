//! Security boundary of the metadata database (ADR 0010, #115): which role
//! the server may run as, and whether its schema is the one this build
//! expects. PostgreSQL is only observed in [`inspect`]; the judgement below
//! is pure, so every combination of capabilities can be tested without I/O.

pub mod inspect;

use std::collections::BTreeSet;
use std::fmt;

/// How the server treats its metadata connection (`BI_METADATA_MODE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MetadataMode {
    /// Default: never migrates, and admits work only for a compatible schema
    /// reached through a least-privilege role.
    #[default]
    Restricted,
    /// Local synthetic development only: migrates on start and tolerates a
    /// privileged role, reporting it with a structured warning.
    Development,
}

impl MetadataMode {
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "restricted" => Some(Self::Restricted),
            "development" => Some(Self::Development),
            _ => None,
        }
    }
}

/// What one role reachable from the connection can do: its session user,
/// its current user, or any role either of them belongs to directly or
/// indirectly, whether through inheritance or `SET ROLE`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // One observed PostgreSQL fact per field.
pub struct ReachableRole {
    pub superuser: bool,
    pub create_role: bool,
    pub create_database: bool,
    pub replication: bool,
    pub bypass_rls: bool,
    pub owns_database: bool,
    pub owns_metadata_schema: bool,
    /// Owns `audit_events` or `audit_head`.
    pub owns_audit_storage: bool,
    pub owns_audit_append: bool,
    pub owns_migrations: bool,
    pub creates_in_metadata_schema: bool,
    /// Any table privilege on the audit storage, including column grants and
    /// grants to `PUBLIC`: only `audit_append` may read or write it.
    pub accesses_audit_storage: bool,
    pub writes_migrations: bool,
    pub grants_audit_append: bool,
    /// A role reserved by PostgreSQL (`pg_` prefix), such as
    /// `pg_execute_server_program` or `pg_read_server_files`: powers over
    /// the server's programs and files that no grant on `bi_meta` shows.
    pub reserved_role: bool,
}

/// What the connection's current user can actually do, for today's handlers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // One required capability per field.
pub struct Effective {
    pub connect: bool,
    pub metadata_schema_usage: bool,
    pub read_migrations: bool,
    pub execute_audit_append: bool,
    /// `SELECT`, `INSERT` and `UPDATE` on `sessions`.
    pub sessions: bool,
    /// `SELECT`, `INSERT` and `DELETE` on `oidc_attempts` (`DELETE` also
    /// permits the `LOCK` that serializes attempts).
    pub oidc_attempts: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedMigration {
    pub version: i64,
    pub success: bool,
    pub checksum: Vec<u8>,
}

/// The migration history as the runtime role can see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum History {
    /// The metadata schema, its audited objects or the migrations table are missing.
    Absent,
    /// It exists, but this role cannot read it.
    Unreadable,
    Applied(Vec<AppliedMigration>),
}

/// Everything [`inspect::facts`] observes; nothing here is a verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub reachable: Vec<ReachableRole>,
    pub effective: Effective,
    pub history: History,
}

/// A capability the runtime role must not have, directly or through any role
/// it can reach. `Display` is the stable code used in logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Forbidden {
    Superuser,
    CreateRole,
    CreateDatabase,
    Replication,
    BypassRls,
    OwnsDatabase,
    OwnsMetadataSchema,
    OwnsAuditStorage,
    OwnsAuditAppend,
    OwnsMigrations,
    CreatesInMetadataSchema,
    AccessesAuditStorage,
    WritesMigrations,
    GrantsAuditAppend,
    ReservedRole,
}

impl Forbidden {
    pub const ALL: [Self; 15] = [
        Self::Superuser,
        Self::CreateRole,
        Self::CreateDatabase,
        Self::Replication,
        Self::BypassRls,
        Self::OwnsDatabase,
        Self::OwnsMetadataSchema,
        Self::OwnsAuditStorage,
        Self::OwnsAuditAppend,
        Self::OwnsMigrations,
        Self::CreatesInMetadataSchema,
        Self::AccessesAuditStorage,
        Self::WritesMigrations,
        Self::GrantsAuditAppend,
        Self::ReservedRole,
    ];

    /// Whether `role` holds this capability.
    #[must_use]
    pub const fn held_by(self, role: &ReachableRole) -> bool {
        match self {
            Self::Superuser => role.superuser,
            Self::CreateRole => role.create_role,
            Self::CreateDatabase => role.create_database,
            Self::Replication => role.replication,
            Self::BypassRls => role.bypass_rls,
            Self::OwnsDatabase => role.owns_database,
            Self::OwnsMetadataSchema => role.owns_metadata_schema,
            Self::OwnsAuditStorage => role.owns_audit_storage,
            Self::OwnsAuditAppend => role.owns_audit_append,
            Self::OwnsMigrations => role.owns_migrations,
            Self::CreatesInMetadataSchema => role.creates_in_metadata_schema,
            Self::AccessesAuditStorage => role.accesses_audit_storage,
            Self::WritesMigrations => role.writes_migrations,
            Self::GrantsAuditAppend => role.grants_audit_append,
            Self::ReservedRole => role.reserved_role,
        }
    }
}

impl fmt::Display for Forbidden {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Superuser => "superuser",
            Self::CreateRole => "create_role",
            Self::CreateDatabase => "create_database",
            Self::Replication => "replication",
            Self::BypassRls => "bypass_rls",
            Self::OwnsDatabase => "owns_database",
            Self::OwnsMetadataSchema => "owns_metadata_schema",
            Self::OwnsAuditStorage => "owns_audit_storage",
            Self::OwnsAuditAppend => "owns_audit_append",
            Self::OwnsMigrations => "owns_migrations",
            Self::CreatesInMetadataSchema => "creates_in_metadata_schema",
            Self::AccessesAuditStorage => "accesses_audit_storage",
            Self::WritesMigrations => "writes_migrations",
            Self::GrantsAuditAppend => "grants_audit_append",
            Self::ReservedRole => "reserved_role",
        })
    }
}

/// A capability today's handlers need and the current user lacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Missing {
    Connect,
    MetadataSchemaUsage,
    ReadMigrations,
    ExecuteAuditAppend,
    Sessions,
    OidcAttempts,
}

impl fmt::Display for Missing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Connect => "missing_connect",
            Self::MetadataSchemaUsage => "missing_metadata_schema_usage",
            Self::ReadMigrations => "missing_read_migrations",
            Self::ExecuteAuditAppend => "missing_execute_audit_append",
            Self::Sessions => "missing_sessions",
            Self::OidcAttempts => "missing_oidc_attempts",
        })
    }
}

/// Why the stored schema is not the one this build embeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemaProblem {
    Absent,
    Unverifiable,
    Failed { version: i64 },
    Unknown { version: i64 },
    ChecksumMismatch { version: i64 },
    Pending { version: i64 },
}

impl fmt::Display for SchemaProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Absent => "schema_absent",
            Self::Unverifiable => "schema_unverifiable",
            Self::Failed { .. } => "migration_failed",
            Self::Unknown { .. } => "migration_unknown",
            Self::ChecksumMismatch { .. } => "migration_checksum_mismatch",
            Self::Pending { .. } => "migration_pending",
        })
    }
}

/// Compares the stored history with the migrations embedded in this build,
/// read-only and with the same rules `SQLx` applies before migrating: a
/// failed migration, an unknown (future) version or a different checksum is
/// never accepted, and every embedded migration must be applied.
///
/// # Errors
/// Returns the first [`SchemaProblem`] found.
pub fn compare_history(history: &History, embedded: &[(i64, &[u8])]) -> Result<(), SchemaProblem> {
    let applied = match history {
        History::Absent => return Err(SchemaProblem::Absent),
        History::Unreadable => return Err(SchemaProblem::Unverifiable),
        History::Applied(applied) => applied,
    };
    if let Some(failed) = applied.iter().find(|migration| !migration.success) {
        return Err(SchemaProblem::Failed {
            version: failed.version,
        });
    }
    for migration in applied {
        match embedded
            .iter()
            .find(|(version, _)| *version == migration.version)
        {
            None => {
                return Err(SchemaProblem::Unknown {
                    version: migration.version,
                });
            }
            Some((_, checksum)) if *checksum != migration.checksum.as_slice() => {
                return Err(SchemaProblem::ChecksumMismatch {
                    version: migration.version,
                });
            }
            Some(_) => {}
        }
    }
    if let Some((version, _)) = embedded.iter().find(|(version, _)| {
        applied
            .iter()
            .all(|migration| migration.version != *version)
    }) {
        return Err(SchemaProblem::Pending { version: *version });
    }
    Ok(())
}

/// The verdict on one set of [`Facts`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assessment {
    pub mode: MetadataMode,
    pub schema: Result<(), SchemaProblem>,
    pub forbidden: BTreeSet<Forbidden>,
    pub missing: BTreeSet<Missing>,
}

impl Assessment {
    #[must_use]
    pub fn of(facts: &Facts, embedded: &[(i64, &[u8])], mode: MetadataMode) -> Self {
        let forbidden = Forbidden::ALL
            .into_iter()
            .filter(|capability| facts.reachable.iter().any(|role| capability.held_by(role)))
            .collect();
        let effective = facts.effective;
        let missing = [
            (effective.connect, Missing::Connect),
            (
                effective.metadata_schema_usage,
                Missing::MetadataSchemaUsage,
            ),
            (effective.read_migrations, Missing::ReadMigrations),
            (effective.execute_audit_append, Missing::ExecuteAuditAppend),
            (effective.sessions, Missing::Sessions),
            (effective.oidc_attempts, Missing::OidcAttempts),
        ]
        .into_iter()
        .filter_map(|(held, capability)| (!held).then_some(capability))
        .collect();
        Self {
            mode,
            schema: compare_history(&facts.history, embedded),
            forbidden,
            missing,
        }
    }

    /// The role holds everything it needs and nothing it must not.
    #[must_use]
    pub fn secure(&self) -> bool {
        self.forbidden.is_empty() && self.missing.is_empty()
    }

    /// Whether application work may be admitted. Only development tolerates
    /// forbidden capabilities; nothing tolerates a missing one or a schema
    /// this build does not expect.
    #[must_use]
    pub fn admits(&self) -> bool {
        self.schema.is_ok()
            && self.missing.is_empty()
            && (self.forbidden.is_empty() || self.mode == MetadataMode::Development)
    }

    /// The first stable reason it is not admitted or not secure, for logs;
    /// `None` when both. A history this role cannot read names the missing
    /// grant, its cause, rather than the unverifiable schema.
    #[must_use]
    pub fn reason(&self) -> Option<String> {
        if let Err(problem) = self.schema
            && problem != SchemaProblem::Unverifiable
        {
            return Some(problem.to_string());
        }
        self.missing
            .first()
            .map(ToString::to_string)
            .or_else(|| self.schema.err().map(|problem| problem.to_string()))
            .or_else(|| self.forbidden.first().map(ToString::to_string))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const EMBEDDED: [(i64, &[u8]); 2] = [(1, b"one"), (2, b"two")];

    fn applied(rows: &[(i64, bool, &[u8])]) -> History {
        History::Applied(
            rows.iter()
                .map(|(version, success, checksum)| AppliedMigration {
                    version: *version,
                    success: *success,
                    checksum: checksum.to_vec(),
                })
                .collect(),
        )
    }

    fn secure_facts() -> Facts {
        Facts {
            reachable: vec![ReachableRole::default()],
            effective: Effective {
                connect: true,
                metadata_schema_usage: true,
                read_migrations: true,
                execute_audit_append: true,
                sessions: true,
                oidc_attempts: true,
            },
            history: applied(&[(1, true, b"one"), (2, true, b"two")]),
        }
    }

    fn with(capability: Forbidden) -> ReachableRole {
        let mut role = ReachableRole::default();
        match capability {
            Forbidden::Superuser => role.superuser = true,
            Forbidden::CreateRole => role.create_role = true,
            Forbidden::CreateDatabase => role.create_database = true,
            Forbidden::Replication => role.replication = true,
            Forbidden::BypassRls => role.bypass_rls = true,
            Forbidden::OwnsDatabase => role.owns_database = true,
            Forbidden::OwnsMetadataSchema => role.owns_metadata_schema = true,
            Forbidden::OwnsAuditStorage => role.owns_audit_storage = true,
            Forbidden::OwnsAuditAppend => role.owns_audit_append = true,
            Forbidden::OwnsMigrations => role.owns_migrations = true,
            Forbidden::CreatesInMetadataSchema => role.creates_in_metadata_schema = true,
            Forbidden::AccessesAuditStorage => role.accesses_audit_storage = true,
            Forbidden::WritesMigrations => role.writes_migrations = true,
            Forbidden::GrantsAuditAppend => role.grants_audit_append = true,
            Forbidden::ReservedRole => role.reserved_role = true,
        }
        role
    }

    #[test]
    fn modes_are_explicit_and_restricted_by_default() {
        assert_eq!(MetadataMode::default(), MetadataMode::Restricted);
        assert_eq!(
            MetadataMode::parse("development"),
            Some(MetadataMode::Development)
        );
        for value in ["", "Restricted", "dev", "production", "local"] {
            assert_eq!(MetadataMode::parse(value), None, "{value}");
        }
    }

    #[test]
    fn a_least_privilege_role_on_the_expected_schema_is_admitted() {
        let assessment = Assessment::of(&secure_facts(), &EMBEDDED, MetadataMode::Restricted);
        assert!(assessment.secure());
        assert!(assessment.admits());
        assert_eq!(assessment.reason(), None);
    }

    #[test]
    fn each_forbidden_capability_alone_is_named_and_refused() {
        for capability in Forbidden::ALL {
            let mut facts = secure_facts();
            // Held by a role reached only indirectly, never by the login itself.
            facts.reachable.push(with(capability));
            let assessment = Assessment::of(&facts, &EMBEDDED, MetadataMode::Restricted);
            assert_eq!(
                assessment.forbidden,
                BTreeSet::from([capability]),
                "{capability}"
            );
            assert!(!assessment.admits(), "{capability}");
            assert_eq!(assessment.reason(), Some(capability.to_string()));
            // Development tolerates it, but still reports it as insecure.
            let development = Assessment::of(&facts, &EMBEDDED, MetadataMode::Development);
            assert!(
                development.admits() && !development.secure(),
                "{capability}"
            );
        }
    }

    #[test]
    fn schema_history_follows_the_sqlx_rules_without_migrating() {
        for (history, expected) in [
            (History::Absent, Err(SchemaProblem::Absent)),
            (History::Unreadable, Err(SchemaProblem::Unverifiable)),
            (applied(&[]), Err(SchemaProblem::Pending { version: 1 })),
            (
                applied(&[(1, true, b"one")]),
                Err(SchemaProblem::Pending { version: 2 }),
            ),
            (
                applied(&[(1, true, b"one"), (2, false, b"two")]),
                Err(SchemaProblem::Failed { version: 2 }),
            ),
            (
                applied(&[(1, true, b"edited"), (2, true, b"two")]),
                Err(SchemaProblem::ChecksumMismatch { version: 1 }),
            ),
            (
                applied(&[(1, true, b"one"), (2, true, b"two"), (3, true, b"future")]),
                Err(SchemaProblem::Unknown { version: 3 }),
            ),
            (applied(&[(1, true, b"one"), (2, true, b"two")]), Ok(())),
        ] {
            assert_eq!(compare_history(&history, &EMBEDDED), expected);
            let mut facts = secure_facts();
            facts.history = history;
            for mode in [MetadataMode::Restricted, MetadataMode::Development] {
                assert_eq!(
                    Assessment::of(&facts, &EMBEDDED, mode).admits(),
                    expected.is_ok()
                );
            }
        }
    }

    fn arbitrary_role() -> impl Strategy<Value = ReachableRole> {
        prop::collection::vec(any::<bool>(), Forbidden::ALL.len()).prop_map(|bits| {
            let mut role = ReachableRole::default();
            for (capability, held) in Forbidden::ALL.into_iter().zip(bits) {
                if held {
                    let flags = with(capability);
                    role = merge(role, flags);
                }
            }
            role
        })
    }

    fn merge(a: ReachableRole, b: ReachableRole) -> ReachableRole {
        ReachableRole {
            superuser: a.superuser || b.superuser,
            create_role: a.create_role || b.create_role,
            create_database: a.create_database || b.create_database,
            replication: a.replication || b.replication,
            bypass_rls: a.bypass_rls || b.bypass_rls,
            owns_database: a.owns_database || b.owns_database,
            owns_metadata_schema: a.owns_metadata_schema || b.owns_metadata_schema,
            owns_audit_storage: a.owns_audit_storage || b.owns_audit_storage,
            owns_audit_append: a.owns_audit_append || b.owns_audit_append,
            owns_migrations: a.owns_migrations || b.owns_migrations,
            creates_in_metadata_schema: a.creates_in_metadata_schema
                || b.creates_in_metadata_schema,
            accesses_audit_storage: a.accesses_audit_storage || b.accesses_audit_storage,
            writes_migrations: a.writes_migrations || b.writes_migrations,
            grants_audit_append: a.grants_audit_append || b.grants_audit_append,
            reserved_role: a.reserved_role || b.reserved_role,
        }
    }

    fn arbitrary_effective() -> impl Strategy<Value = Effective> {
        prop::array::uniform6(any::<bool>()).prop_map(|bits| Effective {
            connect: bits[0],
            metadata_schema_usage: bits[1],
            read_migrations: bits[2],
            execute_audit_append: bits[3],
            sessions: bits[4],
            oidc_attempts: bits[5],
        })
    }

    proptest! {
        /// Restricted mode admits exactly when no reachable role holds any
        /// forbidden capability, the current user holds every required one,
        /// and the schema matches: stated independently of `Assessment`.
        #[test]
        fn restricted_admits_only_a_fully_secure_role(
            reachable in prop::collection::vec(arbitrary_role(), 0..6),
            effective in arbitrary_effective(),
            schema_ok in any::<bool>(),
        ) {
            let history = if schema_ok {
                applied(&[(1, true, b"one"), (2, true, b"two")])
            } else {
                applied(&[(1, true, b"one")])
            };
            let facts = Facts { reachable: reachable.clone(), effective, history };
            let assessment = Assessment::of(&facts, &EMBEDDED, MetadataMode::Restricted);
            let any_forbidden = reachable.iter().any(|role| Forbidden::ALL.iter().any(|c| c.held_by(role)));
            let all_required = effective.connect && effective.metadata_schema_usage
                && effective.read_migrations && effective.execute_audit_append
                && effective.sessions && effective.oidc_attempts;
            prop_assert_eq!(assessment.admits(), !any_forbidden && all_required && schema_ok);
            prop_assert_eq!(assessment.secure(), !any_forbidden && all_required);
            prop_assert_eq!(assessment.reason().is_none(), assessment.secure() && schema_ok);
        }

        /// Gaining any forbidden capability, through any reachable role,
        /// never makes an insecure verdict secure or a refused one admitted.
        #[test]
        fn adding_a_capability_never_helps(
            reachable in prop::collection::vec(arbitrary_role(), 0..6),
            effective in arbitrary_effective(),
            extra in prop::sample::select(Forbidden::ALL.to_vec()),
        ) {
            let before = Facts {
                reachable: reachable.clone(),
                effective,
                history: applied(&[(1, true, b"one"), (2, true, b"two")]),
            };
            let mut after = before.clone();
            after.reachable.push(with(extra));
            let before = Assessment::of(&before, &EMBEDDED, MetadataMode::Restricted);
            let after = Assessment::of(&after, &EMBEDDED, MetadataMode::Restricted);
            prop_assert!(!after.admits() && !after.secure());
            prop_assert!(after.forbidden.is_superset(&before.forbidden));
        }
    }
}
