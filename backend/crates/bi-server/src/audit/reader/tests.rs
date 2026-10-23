//! The reader's verdict on observed facts: only a role that reads the chain
//! and nothing else, on a read-only session, passes.

use proptest::prelude::*;

use super::*;

fn secure() -> ReaderFacts {
    ReaderFacts {
        reachable: vec![ReaderRole::default()],
        effective: ReaderEffective {
            connect: true,
            metadata_schema_usage: true,
            reads_events: true,
            reads_head: true,
            read_only: true,
            objects_present: true,
        },
    }
}

fn with(problem: ReaderProblem) -> ReaderRole {
    let mut role = ReaderRole::default();
    let field = match problem {
        ReaderProblem::OtherRole => &mut role.other_role,
        ReaderProblem::Superuser => &mut role.superuser,
        ReaderProblem::CreateRole => &mut role.create_role,
        ReaderProblem::CreateDatabase => &mut role.create_database,
        ReaderProblem::Replication => &mut role.replication,
        ReaderProblem::BypassRls => &mut role.bypass_rls,
        ReaderProblem::OwnsDatabase => &mut role.owns_database,
        ReaderProblem::OwnsSchema => &mut role.owns_schema,
        ReaderProblem::OwnsAuditStorage => &mut role.owns_audit_storage,
        ReaderProblem::OwnsAuditFunctions => &mut role.owns_audit_functions,
        ReaderProblem::OwnsSessions => &mut role.owns_sessions,
        ReaderProblem::OwnsMigrations => &mut role.owns_migrations,
        ReaderProblem::CreatesInDatabase => &mut role.creates_in_database,
        ReaderProblem::CreatesInSchema => &mut role.creates_in_schema,
        ReaderProblem::WritesAuditStorage => &mut role.writes_audit_storage,
        ReaderProblem::GrantsAuditStorage => &mut role.grants_audit_storage,
        ReaderProblem::ExecutesAuditAppend => &mut role.executes_audit_append,
        ReaderProblem::AccessesSessions => &mut role.accesses_sessions,
        ReaderProblem::WritesMigrations => &mut role.writes_migrations,
        _ => unreachable!("not a capability"),
    };
    *field = true;
    role
}

#[test]
fn a_role_that_only_reads_the_chain_passes() {
    assert_eq!(assess(&secure()), Vec::new());
}

/// Each forbidden capability alone, held by the login or by any role it
/// reaches, is named and refuses the connection.
#[test]
fn each_forbidden_capability_alone_is_named() {
    for problem in ReaderProblem::FORBIDDEN {
        assert!(problem.held_by(&with(problem)), "{problem}");
        for reached in [false, true] {
            let mut facts = secure();
            if reached {
                facts.reachable.push(with(problem));
            } else {
                facts.reachable[0] = with(problem);
            }
            assert_eq!(assess(&facts), vec![problem], "{problem}");
        }
    }
}

/// Takes one required capability away.
type Removal = fn(&mut ReaderEffective);

/// Each missing capability is named: no partial reader.
#[test]
fn each_missing_capability_is_named() {
    let cases: [(Removal, ReaderProblem); 6] = [
        (|e| e.connect = false, ReaderProblem::MissingConnect),
        (
            |e| e.metadata_schema_usage = false,
            ReaderProblem::MissingSchemaUsage,
        ),
        (
            |e| e.reads_events = false,
            ReaderProblem::MissingEventsSelect,
        ),
        (|e| e.reads_head = false, ReaderProblem::MissingHeadSelect),
        (|e| e.read_only = false, ReaderProblem::NotReadOnly),
        (|e| e.objects_present = false, ReaderProblem::ObjectsAbsent),
    ];
    for (remove, problem) in cases {
        let mut facts = secure();
        remove(&mut facts.effective);
        assert_eq!(assess(&facts), vec![problem], "{problem}");
    }
    // Nothing observed is never accepted.
    let mut nothing = secure();
    nothing.reachable.clear();
    assert_eq!(assess(&nothing), vec![ReaderProblem::ObjectsAbsent]);
}

#[test]
fn the_connection_string_is_never_printed() {
    let url = ReaderUrl::new("postgres://reader:secret-7f3a@db.invalid/bi".to_owned());
    let printed = format!("{url:?}");
    assert!(
        !printed.contains("secret-7f3a") && !printed.contains("reader"),
        "{printed}"
    );
    assert!(url.options().is_some());
    assert!(ReaderUrl::new("not a url".to_owned()).options().is_none());
}

fn role() -> impl Strategy<Value = ReaderRole> {
    prop::array::uniform19(any::<bool>()).prop_map(|b| ReaderRole {
        other_role: b[0],
        superuser: b[1],
        create_role: b[2],
        create_database: b[3],
        replication: b[4],
        bypass_rls: b[5],
        owns_database: b[6],
        owns_schema: b[7],
        owns_audit_storage: b[8],
        owns_audit_functions: b[9],
        owns_sessions: b[10],
        owns_migrations: b[11],
        creates_in_database: b[12],
        creates_in_schema: b[13],
        writes_audit_storage: b[14],
        grants_audit_storage: b[15],
        executes_audit_append: b[16],
        accesses_sessions: b[17],
        writes_migrations: b[18],
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// A reader passes exactly when no reachable role holds any forbidden
    /// capability; adding a role or a capability never makes a refused
    /// reader pass.
    #[test]
    fn adding_a_capability_never_helps(
        roles in prop::collection::vec(role(), 1..4),
        extra in role(),
    ) {
        let facts = ReaderFacts { reachable: roles, ..secure() };
        let clean = facts
            .reachable
            .iter()
            .all(|r| ReaderProblem::FORBIDDEN.iter().all(|p| !p.held_by(r)));
        prop_assert_eq!(assess(&facts).is_empty(), clean);
        let mut more = facts.clone();
        more.reachable.push(extra);
        if !assess(&facts).is_empty() {
            prop_assert!(!assess(&more).is_empty());
        }
    }
}
