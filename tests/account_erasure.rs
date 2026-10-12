//! Deleting an account erases what is kept about the person: the engine's own
//! rows, and the rows scripts keep in tables that name their person column.
//!
//! Each case sets up two people and erases one, because "the rows are gone"
//! means nothing unless the other person's rows are still there.

mod common;

use aiwebengine::engine_api::{UserAdminError, delete_user_authorized, execute_native_mcp_tool};
use aiwebengine::repository;
use aiwebengine::script_eval::{EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use aiwebengine::user_repository::{self, UserRole};
use common::setup_env;
use serde_json::{Value, json};

async fn create_user(label: &str) -> String {
    let unique = uuid::Uuid::new_v4();
    user_repository::upsert_user(
        format!("{}-{}@example.com", label, unique),
        Some(format!("Test {}", label)),
        "test".to_string(),
        format!("{}-{}", label, unique),
        "test.example.com".to_string(),
    )
    .await
    .expect("failed to create test user")
}

fn admin() -> UserContext {
    UserContext::admin("erasure-admin".to_string())
}

/// Run `source` in `uri` as an administrator and return its value.
async fn eval(uri: &str, source: String) -> Value {
    let request = EvalRequest {
        timeout_ms: Some(15_000),
        rollback: false,
        ..EvalRequest::new(uri.to_string(), source, admin())
    };
    let report = tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked");
    assert!(report.ok, "evaluation failed: {:?}", report.outcome.error);
    report.outcome.value.unwrap_or(Value::Null)
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_an_account_erases_the_persons_rows_and_nobody_elses() {
    setup_env().await;
    let gone = create_user("gone").await;
    let stays = create_user("stays").await;
    let uri = "test://erasure/notes";
    repository::upsert_script(uri, "function init() {}").expect("script stored");

    // The engine's own rows, for both.
    for person in [&gone, &stays] {
        repository::set_user_properties_item(uri, person, "note", "mine").expect("storage");
        repository::set_user_secret_item(uri, person, "API_KEY", "secret").expect("secret");
    }

    // A script's own tables: notes name their person, a reply refers to a note
    // optionally, a pin requires one.
    eval(
        uri,
        format!(
            r#"
            database.ensureTable("notes", {{
                columns: [{{ name: "owner", type: "text" }}, {{ name: "body", type: "text" }}],
                person: "owner",
            }});
            database.ensureTable("replies", {{
                columns: [
                    {{ name: "author", type: "text" }},
                    {{ name: "note", type: "reference", references: "notes" }},
                ],
                person: "author",
            }});
            database.ensureTable("pins", {{
                columns: [{{ name: "note", type: "reference", references: "notes", nullable: false }}],
            }});
            const gone = database.insert("notes", {{ owner: "{gone}", body: "erase me" }});
            database.insert("notes", {{ owner: "{stays}", body: "keep me" }});
            database.insert("replies", {{ author: "{stays}", note: gone.id }});
            database.insert("pins", {{ note: gone.id }});
            true
            "#
        ),
    )
    .await;

    delete_user_authorized(&admin(), &gone).expect("an administrator may delete an account");

    let left = eval(
        uri,
        r#"({
            notes: database.query("notes").map((r) => r.owner),
            replies: database.query("replies").map((r) => ({ author: r.author, note: r.note })),
            pins: database.query("pins").length,
        })"#
        .to_string(),
    )
    .await;
    assert_eq!(
        left["notes"],
        json!([stays]),
        "only the other person's note: {left}"
    );
    assert_eq!(
        left["replies"],
        json!([{ "author": stays, "note": null }]),
        "an optional reference to an erased row is emptied, not deleted: {left}"
    );
    assert_eq!(
        left["pins"],
        json!(0),
        "a required reference goes with the row it needed: {left}"
    );

    assert_eq!(
        repository::get_user_properties_item(uri, &gone, "note"),
        None
    );
    assert_eq!(
        repository::get_user_secret_item(uri, &gone, "API_KEY"),
        None
    );
    assert_eq!(
        repository::get_user_properties_item(uri, &stays, "note").as_deref(),
        Some("mine")
    );
    assert_eq!(
        repository::get_user_secret_item(uri, &stays, "API_KEY").as_deref(),
        Some("secret")
    );
    assert!(
        user_repository::get_user(&gone).is_err(),
        "the account is gone"
    );
}

/// A person column has to be a text column the table has, or the declaration
/// would promise an erasure that deletes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_person_column_must_be_a_text_column_the_table_has() {
    setup_env().await;
    let uri = "test://erasure/declarations";
    repository::upsert_script(uri, "function init() {}").expect("script stored");

    let out = eval(
        uri,
        r#"
        function attempt(spec) {
            try { database.ensureTable("t", spec); return "accepted"; }
            catch (e) { return String(e.message || e); }
        }
        ({
            missing: attempt({ columns: [{ name: "a", type: "text" }], person: "who" }),
            numeric: attempt({ columns: [{ name: "n", type: "integer" }], person: "n" }),
            text: attempt({ columns: [{ name: "who", type: "text" }], person: "who" }),
        })
        "#
        .to_string(),
    )
    .await;
    assert!(
        out["missing"]
            .as_str()
            .unwrap_or_default()
            .contains("does not have"),
        "{out}"
    );
    assert!(
        out["numeric"]
            .as_str()
            .unwrap_or_default()
            .contains("text column"),
        "{out}"
    );
    assert_eq!(out["text"], json!("accepted"), "{out}");
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_an_account_takes_an_administrator_and_spares_the_last_one() {
    setup_env().await;
    let target = create_user("target").await;

    assert!(matches!(
        delete_user_authorized(&UserContext::authenticated("someone".to_string()), &target),
        Err(UserAdminError::AccessDenied)
    ));
    assert!(user_repository::get_user(&target).is_ok());

    // Make the target the only administrator there is.
    for user in user_repository::list_users().expect("users listed") {
        if user.id != target && user.has_role(&UserRole::Administrator) {
            user_repository::remove_user_role(&user.id, &UserRole::Administrator)
                .expect("role removed");
        }
    }
    user_repository::add_user_role(&target, UserRole::Administrator).expect("role added");
    assert!(matches!(
        delete_user_authorized(&admin(), &target),
        Err(UserAdminError::LastAdministrator)
    ));

    let answer = execute_native_mcp_tool("delete_user", &json!({ "user_id": "nobody" }), &admin());
    let answer = answer.expect("delete_user is a native tool");
    assert!(
        answer.to_string().contains("not found"),
        "an unknown account is named as such: {answer}"
    );
}
