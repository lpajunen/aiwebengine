//! `database`: a script's own tables.

use super::*;
use crate::security::Capability;
use rquickjs::{Function, Result as JsResult, function::Opt};
use tracing::debug;

/// Gives the host namespaces that answer with a JSON string the same shape a
/// `fetch` response has.
pub(super) const DATABASE_PRELUDE: &str = include_str!("../../../assets/database_prelude.js");

/// Turn the arguments a script passed to `database.query` into the options the
/// repository runs it under.
///
/// Every one of them is validated here rather than nearer the statement, so a
/// query that cannot mean what it says is refused before anything runs. Two of
/// those refusals are new: a sort direction that is neither `asc` nor `desc`
/// used to sort ascending, and an unrecognised option key used to be dropped.
/// Both handed back a query that read like the one the script asked for and
/// was not.
pub(super) fn build_query_options(
    limit: Option<i32>,
    order_by: Option<String>,
    order_dir: Option<String>,
    options_json: Option<&str>,
) -> Result<crate::repository::QueryOptions, String> {
    let mut options = crate::repository::QueryOptions {
        limit: limit.map(i64::from),
        order_by,
        ..Default::default()
    };

    if let Some(raw) = order_dir {
        options.order_dir = crate::repository::OrderDirection::parse(&raw)
            .ok_or_else(|| format!("orderDir must be \"asc\" or \"desc\", got \"{}\"", raw))?;
    }

    let Some(raw) = options_json else {
        return Ok(options);
    };
    if raw.trim().is_empty() {
        return Ok(options);
    }

    let parsed: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("Invalid options JSON: {}", e))?;
    let object = parsed.as_object().ok_or_else(|| {
        "options must be a JSON object, for example {\"forUpdate\": true}".to_string()
    })?;

    for (key, value) in object {
        match key.as_str() {
            "forUpdate" => {
                options.for_update = value.as_bool().ok_or_else(|| {
                    format!("options.forUpdate must be true or false, got {}", value)
                })?;
            }
            other => {
                return Err(format!(
                    "Unknown query option '{}'; the supported options are: forUpdate",
                    other
                ));
            }
        }
    }

    Ok(options)
}

/// Reads the schema a script wants a table to have.
///
/// `{ "columns": [{ "name", "type", "nullable"?, "default"?, "references"? }],
/// "uniqueIndexes"?: [["col"]] }`. A `"reference"` column names the table it
/// points at in `references`. `nullable` defaults to true,
/// because a column added to a table that already has rows cannot be `NOT NULL`
/// without a default, and the whole point of this call is that it is safe to
/// make against a table that is already in use.
pub(super) fn parse_table_spec(schema_json: &str) -> Result<crate::repository::TableSpec, String> {
    use std::str::FromStr;

    let parsed: serde_json::Value =
        serde_json::from_str(schema_json).map_err(|e| format!("Invalid schema JSON: {}", e))?;

    let columns = parsed
        .get("columns")
        .and_then(|columns| columns.as_array())
        .ok_or("schema must carry a \"columns\" array")?;

    let mut spec = crate::repository::TableSpec::default();

    for column in columns {
        let name = column
            .get("name")
            .and_then(|name| name.as_str())
            .ok_or("every column needs a \"name\"")?;
        let type_name = column
            .get("type")
            .and_then(|kind| kind.as_str())
            .ok_or_else(|| format!("column \"{}\" needs a \"type\"", name))?;
        let references = column
            .get("references")
            .and_then(|table| table.as_str())
            .map(str::to_string);
        let column_type = if type_name == "reference" {
            if references.is_none() {
                return Err(format!(
                    "column \"{}\" is a reference and needs \"references\": the table it points at",
                    name
                ));
            }
            crate::db_schema_utils::ColumnType::Integer
        } else {
            if references.is_some() {
                return Err(format!(
                    "column \"{}\" has \"references\" but is not of type \"reference\"",
                    name
                ));
            }
            crate::db_schema_utils::ColumnType::from_str(type_name)
                .map_err(|e| format!("column \"{}\": {}", name, e))?
        };

        spec.columns.push(crate::repository::EnsuredColumn {
            name: name.to_string(),
            column_type,
            nullable: column
                .get("nullable")
                .and_then(|nullable| nullable.as_bool())
                .unwrap_or(true),
            default_value: column
                .get("default")
                .and_then(|default| default.as_str())
                .map(str::to_string),
            references,
        });
    }

    if let Some(indexes) = parsed.get("uniqueIndexes").and_then(|i| i.as_array()) {
        for index in indexes {
            let columns = index
                .as_array()
                .ok_or("every entry in \"uniqueIndexes\" must be an array of column names")?
                .iter()
                .map(|column| {
                    column
                        .as_str()
                        .map(str::to_string)
                        .ok_or("index columns must be strings")
                })
                .collect::<Result<Vec<_>, _>>()?;
            spec.unique_indexes.push(columns);
        }
    }

    Ok(spec)
}

impl SecureGlobalContext {
    /// Setup database functions
    pub(super) fn setup_database_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();
        let user_context = self.user_context.clone();

        // Create the database namespace object for schema management
        let database_obj = rquickjs::Object::new(ctx.clone())?;

        // database.ensureTable(tableName, schemaJson) - Converge a table's shape
        let script_uri_ensure = script_uri_owned.clone();
        let user_ctx_ensure = user_context.clone();
        let ensure_table = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  schema_json: String|
                  -> JsResult<String> {
                debug!(
                    "database.ensureTable called for script {} with table: {}",
                    script_uri_ensure, table_name
                );

                if user_ctx_ensure
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(error_answer(
                        "Insufficient permissions for database schema operations",
                    ));
                }

                let spec = match parse_table_spec(&schema_json) {
                    Ok(spec) => spec,
                    Err(e) => return Ok(error_answer(e)),
                };

                match crate::repository::ensure_script_table(&script_uri_ensure, &table_name, &spec)
                {
                    Ok(ensured) => Ok(success_answer(
                        serde_json::to_value(&ensured).unwrap_or_else(|_| serde_json::json!({})),
                    )),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("ensureTable", ensure_table)?;

        // database.dropColumn(tableName, columnName)
        let script_uri_drop_col = script_uri_owned.clone();
        let user_ctx_drop_col = user_context.clone();
        let drop_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String|
                  -> JsResult<String> {
                debug!(
                    "database.dropColumn called for script {} with table: {}, column: {}",
                    script_uri_drop_col, table_name, column_name
                );

                if user_ctx_drop_col
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                match crate::repository::drop_column(
                    &script_uri_drop_col,
                    &table_name,
                    &column_name,
                ) {
                    Ok(existed) => {
                        if existed {
                            Ok(success_answer(serde_json::json!({
                                "tableName": table_name,
                                "columnName": column_name,
                                "dropped": true,
                            })))
                        } else {
                            Ok(success_answer(serde_json::json!({
                                "tableName": table_name,
                                "columnName": column_name,
                                "dropped": false,
                                "message": "Column did not exist",
                            })))
                        }
                    }
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("dropColumn", drop_column)?;

        // database.dropTable(tableName)
        let script_uri_drop = script_uri_owned.clone();
        let user_ctx_drop = user_context.clone();
        let drop_table = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, table_name: String| -> JsResult<String> {
                debug!(
                    "database.dropTable called for script {} with table: {}",
                    script_uri_drop, table_name
                );

                if user_ctx_drop
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                match crate::repository::drop_script_table(&script_uri_drop, &table_name) {
                    Ok(existed) => {
                        if existed {
                            Ok(success_answer(serde_json::json!({
                                "tableName": table_name,
                                "dropped": true,
                            })))
                        } else {
                            Ok(success_answer(serde_json::json!({
                                "tableName": table_name,
                                "dropped": false,
                                "message": "Table did not exist",
                            })))
                        }
                    }
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("dropTable", drop_table)?;

        // query(tableName, filters, limit, orderBy, orderDir, options): the prelude
        // maps `database.query`'s options object onto these positions.
        // filters supports equality {"col": val} and range operators {"col": {"$gt": val, ...}}
        // options supports {"forUpdate": true} to hold the returned rows for
        // the rest of the transaction.
        let script_uri_query = script_uri_owned.clone();
        let user_ctx_query = user_context.clone();
        let query_table = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  filters: Opt<rquickjs::Value<'_>>,
                  limit: Opt<rquickjs::Value<'_>>,
                  order_by: Opt<rquickjs::Value<'_>>,
                  order_dir: Opt<rquickjs::Value<'_>>,
                  options: Opt<rquickjs::Value<'_>>|
                  -> JsResult<String> {
                debug!(
                    "database.query called for script {} on table: {}",
                    script_uri_query, table_name
                );

                if !user_ctx_query.has_capability(&Capability::ReadScriptData) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::ReadScriptData,
                        &user_ctx_query,
                    )));
                }

                let filters_arg: Option<String> = match optional_arg(filters, "filters") {
                    Ok(value) => value,
                    Err(message) => return Ok(error_answer(message)),
                };
                let limit_arg: Option<i32> = match optional_arg(limit, "limit") {
                    Ok(value) => value,
                    Err(message) => return Ok(error_answer(message)),
                };
                let order_by_arg: Option<String> = match optional_arg(order_by, "orderBy") {
                    Ok(value) => value,
                    Err(message) => return Ok(error_answer(message)),
                };
                let order_dir_arg: Option<String> = match optional_arg(order_dir, "orderDir") {
                    Ok(value) => value,
                    Err(message) => return Ok(error_answer(message)),
                };
                let options_arg: Option<String> = match optional_arg(options, "options") {
                    Ok(value) => value,
                    Err(message) => return Ok(error_answer(message)),
                };

                let filters_map = if let Some(filters_str) = filters_arg {
                    match serde_json::from_str::<std::collections::HashMap<String, serde_json::Value>>(
                        &filters_str,
                    ) {
                        Ok(map) => Some(map),
                        Err(e) => {
                            return Ok(error_answer(format!("Invalid filters JSON: {}", e)));
                        }
                    }
                } else {
                    None
                };

                let query_options = match build_query_options(
                    limit_arg,
                    order_by_arg,
                    order_dir_arg,
                    options_arg.as_deref(),
                ) {
                    Ok(parsed) => parsed,
                    Err(message) => return Ok(error_answer(message)),
                };

                match crate::repository::query_table(
                    &script_uri_query,
                    &table_name,
                    filters_map.as_ref(),
                    &query_options,
                ) {
                    Ok(rows) => match serde_json::to_string(&rows) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(error_answer(format!("Serialization error: {}", e))),
                    },
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("query", query_table)?;

        // database.insert(tableName, data) - Insert a row
        let script_uri_insert = script_uri_owned.clone();
        let user_ctx_insert = user_context.clone();
        let insert_row = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, table_name: String, data: String| -> JsResult<String> {
                debug!(
                    "database.insert called for script {} on table: {}",
                    script_uri_insert, table_name
                );

                if !user_ctx_insert.has_capability(&Capability::WriteScriptData) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::WriteScriptData,
                        &user_ctx_insert,
                    )));
                }

                // Parse data from JSON string
                let data_map = match serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&data)
                {
                    Ok(map) => map,
                    Err(e) => return Ok(error_answer(format!("Invalid data JSON: {}", e))),
                };

                match crate::repository::insert_row(&script_uri_insert, &table_name, &data_map) {
                    Ok(row) => match serde_json::to_string(&row) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(error_answer(format!("Serialization error: {}", e))),
                    },
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("insert", insert_row)?;

        // database.update(tableName, id, data) - Update a row
        let script_uri_update = script_uri_owned.clone();
        let user_ctx_update = user_context.clone();
        let update_row = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  id: i32,
                  data: String|
                  -> JsResult<String> {
                debug!(
                    "database.update called for script {} on table: {}, id: {}",
                    script_uri_update, table_name, id
                );

                if !user_ctx_update.has_capability(&Capability::WriteScriptData) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::WriteScriptData,
                        &user_ctx_update,
                    )));
                }

                // Parse data from JSON string
                let data_map = match serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&data)
                {
                    Ok(map) => map,
                    Err(e) => return Ok(error_answer(format!("Invalid data JSON: {}", e))),
                };

                match crate::repository::update_row(&script_uri_update, &table_name, id, &data_map)
                {
                    Ok(row) => match serde_json::to_string(&row) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(error_answer(format!("Serialization error: {}", e))),
                    },
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("update", update_row)?;

        // database.delete(tableName, id) - Delete a row
        let script_uri_delete = script_uri_owned.clone();
        let user_ctx_delete = user_context.clone();
        let delete_row = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, table_name: String, id: i32| -> JsResult<String> {
                debug!(
                    "database.delete called for script {} on table: {}, id: {}",
                    script_uri_delete, table_name, id
                );

                if !user_ctx_delete.has_capability(&Capability::WriteScriptData) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::WriteScriptData,
                        &user_ctx_delete,
                    )));
                }

                match crate::repository::delete_row(&script_uri_delete, &table_name, id) {
                    Ok(deleted) => Ok(success_answer(serde_json::json!({ "deleted": deleted }))),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("delete", delete_row)?;

        // database.upsert(tableName, keyColumns, data)
        // INSERT … ON CONFLICT DO UPDATE — atomically insert or update by key
        let script_uri_upsert = script_uri_owned.clone();
        let user_ctx_upsert = user_context.clone();
        let upsert_row = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  key_columns_json: String,
                  data: String|
                  -> JsResult<String> {
                debug!(
                    "database.upsert called for script {} on table: {}",
                    script_uri_upsert, table_name
                );

                if !user_ctx_upsert.has_capability(&Capability::WriteScriptData) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::WriteScriptData,
                        &user_ctx_upsert,
                    )));
                }

                // key_columns is a JSON array of strings, or a single string
                let key_cols: Vec<String> = match serde_json::from_str::<serde_json::Value>(
                    &key_columns_json,
                ) {
                    Ok(serde_json::Value::Array(arr)) => arr
                        .into_iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect(),
                    Ok(serde_json::Value::String(s)) => vec![s],
                    _ => {
                        return Ok("{\"error\": \"keyColumns must be a JSON array of strings or a single string\"}".to_string());
                    }
                };

                let data_map = match serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&data)
                {
                    Ok(map) => map,
                    Err(e) => return Ok(error_answer(format!("Invalid data JSON: {}", e))),
                };

                match crate::repository::upsert_row(
                    &script_uri_upsert,
                    &table_name,
                    &key_cols,
                    &data_map,
                ) {
                    Ok(row) => match serde_json::to_string(&row) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(error_answer(format!("Serialization error: {}", e))),
                    },
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("upsert", upsert_row)?;

        // database.deleteWhere(tableName, filters)
        // Bulk-delete rows matching filter conditions (equality + range operators)
        let script_uri_dw = script_uri_owned.clone();
        let user_ctx_dw = user_context.clone();
        let delete_where = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  filters: String|
                  -> JsResult<String> {
                debug!(
                    "database.deleteWhere called for script {} on table: {}",
                    script_uri_dw, table_name
                );

                if !user_ctx_dw.has_capability(&Capability::WriteScriptData) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::WriteScriptData,
                        &user_ctx_dw,
                    )));
                }

                let filters_map = match serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&filters)
                {
                    Ok(map) => map,
                    Err(e) => return Ok(error_answer(format!("Invalid filters JSON: {}", e))),
                };

                match crate::repository::delete_where(&script_uri_dw, &table_name, &filters_map) {
                    Ok(count) => Ok(success_answer(serde_json::json!({ "deleted": count }))),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("deleteWhere", delete_where)?;

        // Transaction management functions

        // beginTransaction(timeoutMs?): start a transaction, or a savepoint inside one.
        // These three are the host half of `database.transaction(fn)`.
        let begin_transaction = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, timeout_ms: Opt<u64>| -> JsResult<String> {
                match crate::database::Database::begin_transaction(timeout_ms.0) {
                    Ok(guard) => {
                        // The transaction has to outlive this call: the script
                        // expects it open on the next line, and the handler
                        // boundary commits or rolls it back. Dropping the guard
                        // here would roll it back immediately instead, leaving
                        // every write the script went on to make outside any
                        // transaction and nothing for `rollbackTransaction` to
                        // undo.
                        guard.release();
                        Ok("{\"success\": true, \"message\": \"Transaction started\"}".to_string())
                    }
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("beginTransaction", begin_transaction)?;

        // commitTransaction(): commit, or release the innermost savepoint
        let commit_transaction = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                match crate::database::Database::commit_transaction() {
                    Ok(()) => Ok(
                        "{\"success\": true, \"message\": \"Transaction committed\"}".to_string(),
                    ),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("commitTransaction", commit_transaction)?;

        // rollbackTransaction(): roll back, or to the innermost savepoint
        let rollback_transaction = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                match crate::database::Database::rollback_transaction() {
                    Ok(()) => Ok(
                        "{\"success\": true, \"message\": \"Transaction rolled back\"}".to_string(),
                    ),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("rollbackTransaction", rollback_transaction)?;

        // Installed under a private name: the prelude below builds `database`
        // from it, turning each JSON answer into a value and each `{error}`
        // into a thrown error.
        global.set("__hostDatabase", database_obj)?;

        crate::bytecode::eval_program(ctx, "engine://database-prelude", DATABASE_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "database",
                    "prelude",
                    &format!("database prelude failed to load: {}", e),
                )
            },
        )?;

        debug!(
            "database JavaScript API initialized for script: {}",
            script_uri
        );

        Ok(())
    }
}

#[cfg(test)]
mod query_option_tests {
    use super::build_query_options;
    use crate::repository::OrderDirection;

    #[test]
    fn omitting_everything_gives_the_defaults() {
        let options = build_query_options(None, None, None, None).expect("defaults are valid");

        assert_eq!(options.limit, None);
        assert_eq!(options.order_by, None);
        assert_eq!(options.order_dir, OrderDirection::Ascending);
        assert!(!options.for_update);
    }

    #[test]
    fn the_arguments_land_where_the_query_will_look_for_them() {
        let options = build_query_options(
            Some(25),
            Some("ts".to_string()),
            Some("desc".to_string()),
            Some(r#"{"forUpdate": true}"#),
        )
        .expect("a fully specified query is valid");

        assert_eq!(options.limit, Some(25));
        assert_eq!(options.order_by.as_deref(), Some("ts"));
        assert_eq!(options.order_dir, OrderDirection::Descending);
        assert!(options.for_update);
    }

    #[test]
    fn a_sort_direction_that_is_neither_is_refused() {
        // It used to sort ascending. A script asking for "descending" got the
        // opposite of what it asked for, in silence.
        let error = build_query_options(None, None, Some("descending".to_string()), None)
            .expect_err("'descending' is not a direction");

        assert!(
            error.contains("descending") && error.contains("desc"),
            "the refusal should show what was passed and what is accepted: {error}"
        );

        for accepted in ["asc", "ASC", "desc", "DESC", " Desc "] {
            assert!(
                build_query_options(None, None, Some(accepted.to_string()), None).is_ok(),
                "{accepted} should be accepted"
            );
        }
    }

    #[test]
    fn an_unrecognised_option_is_refused_rather_than_dropped() {
        // Dropping it would hand back an unguarded query to a caller who asked
        // for a guarded one — this option's whole failure mode, in silence.
        let error = build_query_options(None, None, None, Some(r#"{"forupdate": true}"#))
            .expect_err("a misspelled key is not an option");

        assert!(
            error.contains("forupdate") && error.contains("forUpdate"),
            "the refusal should name both what was passed and what is supported: {error}"
        );
    }

    #[test]
    fn an_option_of_the_wrong_type_is_refused() {
        let error = build_query_options(None, None, None, Some(r#"{"forUpdate": "yes"}"#))
            .expect_err("a string is not a boolean");
        assert!(error.contains("true or false"), "{error}");

        let error = build_query_options(None, None, None, Some("[]"))
            .expect_err("an array is not an options object");
        assert!(error.contains("JSON object"), "{error}");

        let error = build_query_options(None, None, None, Some("{not json"))
            .expect_err("this is not JSON at all");
        assert!(error.contains("Invalid options JSON"), "{error}");
    }

    #[test]
    fn an_empty_options_string_is_the_same_as_none() {
        // A script building the argument conditionally can end up passing "".
        let options =
            build_query_options(None, None, None, Some("   ")).expect("blank is not an error");
        assert!(!options.for_update);
    }
}
