//! Compare PostgreSQL's own deparsed baseline on the same server/version.
//!
//! Ownership and ACLs are deployment policy, so they are not frozen here. Every
//! schema property enforced by the historical SQL is compared, and unsupported
//! relation/type objects fail closed instead of being silently adopted.

use sqlx::PgConnection;

use super::{Failure, MigrationError};

#[derive(Clone, Copy)]
pub(super) enum Inventory {
    Empty,
    Existing {
        sqlx: bool,
        idempotency: bool,
        reminders: bool,
    },
}

pub(super) async fn inventory(connection: &mut PgConnection) -> Result<Inventory, MigrationError> {
    let public_exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_namespace WHERE nspname='public')",
    )
    .fetch_one(&mut *connection)
    .await?;
    let relations: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT n.nspname,c.relname,c.relkind::text FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname !~ '^pg_' AND n.nspname <> 'information_schema' AND c.relkind NOT IN ('i','I','t') ORDER BY n.nspname,c.relname",
    )
    .fetch_all(&mut *connection)
    .await?;
    let unsupported_types: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_type t JOIN pg_catalog.pg_namespace n ON n.oid=t.typnamespace WHERE n.nspname='public' AND t.typrelid=0 AND t.typelem=0)",
    )
    .fetch_one(&mut *connection)
    .await?;
    if !public_exists
        || unsupported_types
        || relations
            .iter()
            .any(|(schema, _, kind)| schema != "public" || kind != "r")
    {
        return Err(MigrationError::invalid(Failure::UnsupportedDatabase));
    }
    let names: Vec<&str> = relations.iter().map(|(_, name, _)| name.as_str()).collect();
    match names.as_slice() {
        [] => {
            // An empty-looking database with an orphan function is not fresh.
            let has_routines: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_catalog.pg_proc p JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname='public')",
            )
            .fetch_one(connection)
            .await?;
            if has_routines {
                return Err(MigrationError::invalid(Failure::UnsupportedDatabase));
            }
            Ok(Inventory::Empty)
        }
        ["work_order_counters", "work_order_migrations", "work_orders"] => {
            Ok(Inventory::Existing {
                sqlx: false,
                idempotency: false,
                reminders: false,
            })
        }
        ["_sqlx_migrations", "work_order_counters", "work_order_migrations", "work_orders"] => {
            Ok(Inventory::Existing {
                sqlx: true,
                idempotency: false,
                reminders: false,
            })
        }
        ["work_order_counters", "work_order_create_idempotency", "work_order_migrations", "work_orders"] => {
            Ok(Inventory::Existing {
                sqlx: false,
                idempotency: true,
                reminders: false,
            })
        }
        ["_sqlx_migrations", "work_order_counters", "work_order_create_idempotency", "work_order_migrations", "work_orders"] => {
            Ok(Inventory::Existing {
                sqlx: true,
                idempotency: true,
                reminders: false,
            })
        }
        ["work_order_counters", "work_order_create_idempotency", "work_order_migrations", "work_order_reminder_intents", "work_orders"] => {
            Ok(Inventory::Existing {
                sqlx: false,
                idempotency: true,
                reminders: true,
            })
        }
        ["_sqlx_migrations", "work_order_counters", "work_order_create_idempotency", "work_order_migrations", "work_order_reminder_intents", "work_orders"] => {
            Ok(Inventory::Existing {
                sqlx: true,
                idempotency: true,
                reminders: true,
            })
        }
        _ => Err(MigrationError::invalid(Failure::UnsupportedDatabase)),
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct Snapshot(Vec<Vec<String>>);

pub(super) async fn snapshot(
    connection: &mut PgConnection,
    schema: &str,
    include_sqlx: bool,
) -> Result<Snapshot, MigrationError> {
    // Only pg_get_* output receives identifier normalization. In particular raw
    // function bodies/configuration are compared byte-for-byte and never rewritten.
    let mut parts = Vec::with_capacity(SIGNATURES.len());
    for statement in SIGNATURES {
        parts.push(
            sqlx::query_scalar::<_, String>(statement)
                .bind(schema)
                .bind(include_sqlx)
                .fetch_all(&mut *connection)
                .await?,
        );
    }
    Ok(Snapshot(parts))
}

const SIGNATURES: &[&str] = &[
    // Relation storage/security flags and unsupported table-level options.
    r#"
SELECT pg_catalog.jsonb_build_object(
 'name',c.relname,'kind',c.relkind,'persistence',c.relpersistence,
 'access_method',am.amname,'options',c.reloptions,'replica_identity',c.relreplident,
 'row_security',c.relrowsecurity,'force_row_security',c.relforcerowsecurity,
 'is_partition',c.relispartition,'has_subclasses',c.relhassubclass,
 'partition_key',pg_catalog.pg_get_partkeydef(c.oid))::text
FROM pg_catalog.pg_class c
JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
LEFT JOIN pg_catalog.pg_am am ON am.oid=c.relam
WHERE n.nspname=$1 AND c.relkind NOT IN ('i','I','t')
 AND ($2 OR c.relname <> '_sqlx_migrations')
ORDER BY c.relname
"#,
    // Attnum/drop markers also reject a drop-and-readd which merely looks similar.
    r#"
SELECT pg_catalog.jsonb_build_object(
 'table',c.relname,'number',a.attnum,'name',a.attname,
 'type',pg_catalog.format_type(a.atttypid,a.atttypmod),
 'not_null',a.attnotnull,'identity',a.attidentity,'generated',a.attgenerated,
 'dimensions',a.attndims,'dropped',a.attisdropped,'local',a.attislocal,
 'inherited',a.attinhcount,'storage',a.attstorage,
 'collation_schema',cn.nspname,'collation',co.collname,
 'default',pg_catalog.replace(pg_catalog.pg_get_expr(d.adbin,d.adrelid),pg_catalog.quote_ident($1::text)||'.','baseline.'))::text
FROM pg_catalog.pg_attribute a
JOIN pg_catalog.pg_class c ON c.oid=a.attrelid
JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum
LEFT JOIN pg_catalog.pg_collation co ON co.oid=a.attcollation
LEFT JOIN pg_catalog.pg_namespace cn ON cn.oid=co.collnamespace
WHERE n.nspname=$1 AND c.relkind IN ('r','p','f') AND a.attnum>0
 AND ($2 OR c.relname <> '_sqlx_migrations')
ORDER BY c.relname,a.attnum
"#,
    r#"
SELECT pg_catalog.jsonb_build_object(
 'table',c.relname,'name',co.conname,'type',co.contype,
 'deferrable',co.condeferrable,'deferred',co.condeferred,'validated',co.convalidated,
 'no_inherit',co.connoinherit,'local',co.conislocal,'inherited',co.coninhcount,
 'definition',pg_catalog.replace(pg_catalog.pg_get_constraintdef(co.oid,false),pg_catalog.quote_ident($1::text)||'.','baseline.'))::text
FROM pg_catalog.pg_constraint co
JOIN pg_catalog.pg_class c ON c.oid=co.conrelid
JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
WHERE n.nspname=$1 AND ($2 OR c.relname <> '_sqlx_migrations')
ORDER BY c.relname,co.conname
"#,
    r#"
SELECT pg_catalog.jsonb_build_object(
 'table',c.relname,'name',ic.relname,'unique',i.indisunique,'primary',i.indisprimary,
 'exclusion',i.indisexclusion,'immediate',i.indimmediate,
 'valid',i.indisvalid,'ready',i.indisready,'live',i.indislive,
 'clustered',i.indisclustered,'replica_identity',i.indisreplident,
 'options',ic.reloptions,'persistence',ic.relpersistence,
 'definition',pg_catalog.replace(pg_catalog.pg_get_indexdef(i.indexrelid,0,false),pg_catalog.quote_ident($1::text)||'.','baseline.'))::text
FROM pg_catalog.pg_index i
JOIN pg_catalog.pg_class c ON c.oid=i.indrelid
JOIN pg_catalog.pg_class ic ON ic.oid=i.indexrelid
JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
WHERE n.nspname=$1 AND ($2 OR c.relname <> '_sqlx_migrations')
ORDER BY c.relname,ic.relname
"#,
    r#"
SELECT pg_catalog.jsonb_build_object(
 'name',p.proname,'kind',p.prokind,'language',l.lanname,
 'arguments',pg_catalog.pg_get_function_identity_arguments(p.oid),
 'result',pg_catalog.pg_get_function_result(p.oid),
 'source',p.prosrc,'binary',p.probin,'config',p.proconfig,
 'security_definer',p.prosecdef,'strict',p.proisstrict,
 'volatility',p.provolatile,'parallel',p.proparallel,'leakproof',p.proleakproof,
 'set_returning',p.proretset,'cost',p.procost,'rows',p.prorows,
 'argument_names',p.proargnames,'argument_modes',p.proargmodes,
 'default_arguments',pg_catalog.pg_get_expr(p.proargdefaults,0),
 'support',p.prosupport::text)::text
FROM pg_catalog.pg_proc p
JOIN pg_catalog.pg_namespace n ON n.oid=p.pronamespace
JOIN pg_catalog.pg_language l ON l.oid=p.prolang
WHERE n.nspname=$1 AND ($2 OR true)
ORDER BY p.proname,pg_catalog.pg_get_function_identity_arguments(p.oid)
"#,
    // PostgreSQL embeds OIDs in FK trigger names. Compare the stable constraint
    // name and trigger event/type instead, retaining definition/enabled checks.
    r#"
SELECT pg_catalog.jsonb_build_object(
 'table',c.relname,
 'name',CASE WHEN t.tgisinternal AND co.contype='f' THEN 'internal_'||co.conname||'_'||t.tgtype ELSE t.tgname END,
 'type',t.tgtype,'enabled',t.tgenabled,
 'internal',t.tgisinternal,'arguments',pg_catalog.encode(t.tgargs,'hex'),
 'attributes',t.tgattr::text,'old_table',t.tgoldtable,'new_table',t.tgnewtable,
 'definition',pg_catalog.replace(
   CASE WHEN t.tgisinternal AND co.contype='f' THEN pg_catalog.replace(pg_catalog.pg_get_triggerdef(t.oid,false),pg_catalog.quote_ident(t.tgname),'internal_'||co.conname||'_'||t.tgtype)
   ELSE pg_catalog.pg_get_triggerdef(t.oid,false) END,
   pg_catalog.quote_ident($1::text)||'.','baseline.'))::text
FROM pg_catalog.pg_trigger t
JOIN pg_catalog.pg_class c ON c.oid=t.tgrelid
JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
LEFT JOIN pg_catalog.pg_constraint co ON co.oid=t.tgconstraint
WHERE n.nspname=$1 AND ($2 OR c.relname <> '_sqlx_migrations')
ORDER BY c.relname,CASE WHEN t.tgisinternal AND co.contype='f' THEN 'internal_'||co.conname||'_'||t.tgtype ELSE t.tgname END
"#,
    r#"
SELECT pg_catalog.jsonb_build_object(
 'table',c.relname,'name',p.polname,'command',p.polcmd,'permissive',p.polpermissive,
 'roles',p.polroles::text,
 'using',pg_catalog.replace(pg_catalog.pg_get_expr(p.polqual,p.polrelid),pg_catalog.quote_ident($1::text)||'.','baseline.'),
 'check',pg_catalog.replace(pg_catalog.pg_get_expr(p.polwithcheck,p.polrelid),pg_catalog.quote_ident($1::text)||'.','baseline.'))::text
FROM pg_catalog.pg_policy p
JOIN pg_catalog.pg_class c ON c.oid=p.polrelid
JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
WHERE n.nspname=$1 AND ($2 OR c.relname <> '_sqlx_migrations')
ORDER BY c.relname,p.polname
"#,
    r#"
SELECT pg_catalog.jsonb_build_object(
 'table',c.relname,'name',r.rulename,
 'definition',pg_catalog.replace(pg_catalog.pg_get_ruledef(r.oid,false),pg_catalog.quote_ident($1::text)||'.','baseline.'))::text
FROM pg_catalog.pg_rewrite r
JOIN pg_catalog.pg_class c ON c.oid=r.ev_class
JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace
WHERE n.nspname=$1 AND ($2 OR c.relname <> '_sqlx_migrations')
ORDER BY c.relname,r.rulename
"#,
    r#"
SELECT pg_catalog.jsonb_build_object(
 'child',child.relname,'parent_schema',pn.nspname,'parent',parent.relname,
 'sequence',i.inhseqno)::text
FROM pg_catalog.pg_inherits i
JOIN pg_catalog.pg_class child ON child.oid=i.inhrelid
JOIN pg_catalog.pg_namespace n ON n.oid=child.relnamespace
JOIN pg_catalog.pg_class parent ON parent.oid=i.inhparent
JOIN pg_catalog.pg_namespace pn ON pn.oid=parent.relnamespace
WHERE n.nspname=$1 AND ($2 OR child.relname <> '_sqlx_migrations')
ORDER BY child.relname,i.inhseqno
"#,
];
