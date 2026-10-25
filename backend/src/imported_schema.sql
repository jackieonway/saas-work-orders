-- Ordered schema_material contract used by authenticated UUID import proofs.
-- Catalog only: no row scans, DDL, migration execution or ownership/ACL material.
WITH
user_schemas AS MATERIALIZED (
    SELECT oid, nspname
    FROM pg_namespace
    WHERE nspname <> 'information_schema'
      AND left(nspname, 3) <> 'pg_'
),
catalog_items AS MATERIALIZED (
    SELECT 'relation'::text AS kind, n.nspname AS schema_name,
           c.relname AS object_name, ''::text AS sub_name,
           jsonb_build_object(
               'kind', 'relation', 'schema', n.nspname, 'name', c.relname,
               'relation_kind', c.relkind, 'persistence', c.relpersistence,
               'is_partition', c.relispartition,
               'partition_key', CASE WHEN c.relkind = 'p' THEN pg_get_partkeydef(c.oid) END,
               'partition_bound', pg_get_expr(c.relpartbound, c.oid),
               'parents', COALESCE((SELECT jsonb_agg(jsonb_build_object(
                   'schema', pn.nspname, 'table', pc.relname) ORDER BY i.inhseqno)
                   FROM pg_inherits i JOIN pg_class pc ON pc.oid = i.inhparent
                   JOIN pg_namespace pn ON pn.oid = pc.relnamespace
                   WHERE i.inhrelid = c.oid), '[]'::jsonb),
               'row_security', c.relrowsecurity, 'force_row_security', c.relforcerowsecurity,
               'replica_identity', c.relreplident, 'options', c.reloptions,
               'access_method', am.amname
           ) AS item
    FROM pg_class c JOIN user_schemas n ON n.oid = c.relnamespace
    LEFT JOIN pg_am am ON am.oid = c.relam
    WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f', 'S')
    UNION ALL
    SELECT 'column', n.nspname, c.relname, lpad(a.attnum::text, 6, '0'),
           jsonb_build_object(
               'kind', 'column', 'schema', n.nspname, 'table', c.relname,
               'position', a.attnum, 'column', a.attname,
               'type', format_type(a.atttypid, a.atttypmod),
               'not_null', a.attnotnull, 'identity', a.attidentity,
               'generated', a.attgenerated, 'dimensions', a.attndims,
               'storage', a.attstorage, 'local', a.attislocal,
               'inherited', a.attinhcount, 'collation_schema', cn.nspname,
               'collation', co.collname, 'default', pg_get_expr(d.adbin, d.adrelid)
           )
    FROM pg_class c JOIN user_schemas n ON n.oid = c.relnamespace
    JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
    LEFT JOIN pg_attrdef d ON d.adrelid = c.oid AND d.adnum = a.attnum
    LEFT JOIN pg_collation co ON co.oid = a.attcollation
    LEFT JOIN pg_namespace cn ON cn.oid = co.collnamespace
    WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f')
    UNION ALL
    SELECT 'constraint', n.nspname, COALESCE(c.relname, ''), k.conname,
           jsonb_build_object(
               'kind', 'constraint', 'schema', n.nspname, 'table', c.relname,
               'name', k.conname, 'type', k.contype,
               'domain', CASE WHEN k.contypid <> 0 THEN format_type(k.contypid, NULL) END,
               'definition', pg_get_constraintdef(k.oid, false),
               'columns', (SELECT jsonb_agg(a.attname ORDER BY key.position)
                   FROM unnest(k.conkey) WITH ORDINALITY AS key(attnum, position)
                   JOIN pg_attribute a ON a.attrelid = k.conrelid AND a.attnum = key.attnum),
               'referenced_schema', rn.nspname, 'referenced_table', rc.relname,
               'referenced_columns', (SELECT jsonb_agg(a.attname ORDER BY key.position)
                   FROM unnest(k.confkey) WITH ORDINALITY AS key(attnum, position)
                   JOIN pg_attribute a ON a.attrelid = k.confrelid AND a.attnum = key.attnum),
               'validated', k.convalidated, 'deferrable', k.condeferrable,
               'deferred', k.condeferred, 'no_inherit', k.connoinherit
           )
    FROM pg_constraint k JOIN user_schemas n ON n.oid = k.connamespace
    LEFT JOIN pg_class c ON c.oid = k.conrelid
    LEFT JOIN pg_class rc ON rc.oid = k.confrelid
    LEFT JOIN pg_namespace rn ON rn.oid = rc.relnamespace
    UNION ALL
    SELECT 'index', n.nspname, c.relname, ic.relname,
           jsonb_build_object(
               'kind', 'index', 'schema', n.nspname, 'table', c.relname,
               'name', ic.relname, 'definition', pg_get_indexdef(i.indexrelid),
               'unique', i.indisunique, 'primary', i.indisprimary,
               'valid', i.indisvalid, 'ready', i.indisready,
               'replica_identity', i.indisreplident
           )
    FROM pg_index i JOIN pg_class c ON c.oid = i.indrelid
    JOIN pg_class ic ON ic.oid = i.indexrelid
    JOIN user_schemas n ON n.oid = c.relnamespace
    UNION ALL
    SELECT 'trigger', n.nspname, c.relname, t.tgname,
           jsonb_build_object(
               'kind', 'trigger', 'schema', n.nspname, 'table', c.relname,
               'name', t.tgname, 'internal', t.tgisinternal,
               'enabled', t.tgenabled, 'definition', pg_get_triggerdef(t.oid, false)
           )
    FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid
    JOIN user_schemas n ON n.oid = c.relnamespace
    UNION ALL
    SELECT 'view', n.nspname, c.relname, '', jsonb_build_object(
               'kind', 'view', 'schema', n.nspname, 'name', c.relname,
               'materialized', c.relkind = 'm', 'definition', pg_get_viewdef(c.oid, false)
           )
    FROM pg_class c JOIN user_schemas n ON n.oid = c.relnamespace
    WHERE c.relkind IN ('v', 'm')
    UNION ALL
    SELECT 'function', n.nspname, p.proname, pg_get_function_identity_arguments(p.oid),
           jsonb_build_object(
               'kind', 'function', 'schema', n.nspname, 'name', p.proname,
               'routine_kind', p.prokind,
               'arguments', pg_get_function_identity_arguments(p.oid),
               'definition', pg_get_functiondef(p.oid),
               'security_definer', p.prosecdef, 'volatility', p.provolatile,
               'parallel', p.proparallel, 'configuration', p.proconfig
           )
    FROM pg_proc p JOIN user_schemas n ON n.oid = p.pronamespace
    WHERE p.prokind IN ('f', 'p', 'w')
    UNION ALL
    SELECT 'type', n.nspname, t.typname, '', jsonb_build_object(
               'kind', 'type', 'schema', n.nspname, 'name', t.typname,
               'type_kind', t.typtype, 'category', t.typcategory,
               'base_type', CASE WHEN t.typbasetype <> 0 THEN format_type(t.typbasetype, t.typtypmod) END,
               'element_type', CASE WHEN t.typelem <> 0 THEN format_type(t.typelem, NULL) END,
               'not_null', t.typnotnull, 'default', t.typdefault,
               'enum_labels', COALESCE((SELECT jsonb_agg(e.enumlabel ORDER BY e.enumsortorder)
                   FROM pg_enum e WHERE e.enumtypid = t.oid), '[]'::jsonb)
           )
    FROM pg_type t JOIN user_schemas n ON n.oid = t.typnamespace
    UNION ALL
    SELECT 'policy', n.nspname, c.relname, p.polname, jsonb_build_object(
               'kind', 'policy', 'schema', n.nspname, 'table', c.relname,
               'name', p.polname, 'command', p.polcmd, 'permissive', p.polpermissive,
               'roles', (SELECT jsonb_agg(CASE WHEN role_oid = 0 THEN 'public'
                                             ELSE pg_get_userbyid(role_oid) END ORDER BY role_oid)
                         FROM unnest(p.polroles) AS roles(role_oid)),
               'using', pg_get_expr(p.polqual, p.polrelid),
               'with_check', pg_get_expr(p.polwithcheck, p.polrelid)
           )
    FROM pg_policy p JOIN pg_class c ON c.oid = p.polrelid
    JOIN user_schemas n ON n.oid = c.relnamespace
    UNION ALL
    SELECT 'sequence', n.nspname, c.relname, '', jsonb_build_object(
               'kind', 'sequence', 'schema', n.nspname, 'name', c.relname,
               'type', format_type(s.seqtypid, NULL), 'start', s.seqstart,
               'increment', s.seqincrement, 'min', s.seqmin, 'max', s.seqmax,
               'cache', s.seqcache, 'cycle', s.seqcycle
           )
    FROM pg_sequence s JOIN pg_class c ON c.oid = s.seqrelid
    JOIN user_schemas n ON n.oid = c.relnamespace
)
SELECT COALESCE(jsonb_agg(item ORDER BY kind, schema_name, object_name, sub_name),
                '[]'::jsonb)
FROM catalog_items;
