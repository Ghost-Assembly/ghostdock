//! Credentials, repositories, Git-backed stacks, and sealed environments.

use shared::deployment::SourceKind;
use sqlx::Row as _;
use store::Store;
use store::hosts::LOCAL_HOST_ID;

const TOKEN: &str = "ghp_a_real_looking_token";

async fn store() -> Store {
    Store::open_in_memory().await.expect("store")
}

#[tokio::test]
async fn a_credential_secret_is_not_stored_in_the_clear() {
    let s = store().await;
    s.credential_create("github", "x-access-token", TOKEN)
        .await
        .unwrap();

    // Read the raw column: the point is what is on disk, not what the API
    // hands back.
    let stored: String = sqlx::query("SELECT secret_sealed FROM credentials")
        .fetch_one(s.pool())
        .await
        .unwrap()
        .get(0);

    assert!(
        !stored.contains(TOKEN),
        "the token is sitting in the database: {stored}"
    );
    assert!(stored.starts_with("v1:"));
    assert_eq!(
        s.credential_secret(1).await.unwrap(),
        Some(("x-access-token".to_owned(), TOKEN.to_owned()))
    );
}

#[tokio::test]
async fn listing_credentials_never_returns_a_secret() {
    let s = store().await;
    s.credential_create("github", "x-access-token", TOKEN)
        .await
        .unwrap();

    let listed = s.credentials_list().await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "github");

    let rendered = format!("{listed:?}");
    assert!(
        !rendered.contains(TOKEN),
        "a secret reached a Debug string: {rendered}"
    );
}

#[tokio::test]
async fn a_repository_reports_its_credential_by_name() {
    let s = store().await;
    let cred = s.credential_create("github", "x", TOKEN).await.unwrap();
    let repo = s
        .repo_create("https://example.invalid/stacks.git", Some(cred.id))
        .await
        .unwrap();

    assert_eq!(repo.credential_name.as_deref(), Some("github"));
    assert_eq!(s.repos_list().await.unwrap(), vec![repo]);
}

#[tokio::test]
async fn deleting_a_credential_leaves_the_repository_in_place() {
    // Losing a token should break the next poll with an explanation, not
    // make the repository and its stacks disappear.
    let s = store().await;
    let cred = s.credential_create("github", "x", TOKEN).await.unwrap();
    let repo = s
        .repo_create("https://example.invalid/s.git", Some(cred.id))
        .await
        .unwrap();

    s.credential_delete(cred.id).await.unwrap();

    let after = s.repo_by_id(repo.id).await.unwrap().expect("repo survives");
    assert_eq!(after.credential_id, None);
}

#[tokio::test]
async fn a_repository_in_use_cannot_be_deleted() {
    let s = store().await;
    let repo = s
        .repo_create("https://example.invalid/s.git", None)
        .await
        .unwrap();
    s.stack_create_git(
        LOCAL_HOST_ID,
        "blog",
        "Blog",
        repo.id,
        "refs/heads/main",
        "compose/blog.yml",
    )
    .await
    .unwrap();

    assert!(
        matches!(s.repo_delete(repo.id).await, Err(store::Error::InUse)),
        "deleting a repository must not silently take its stacks with it"
    );
}

#[tokio::test]
async fn a_git_stack_carries_its_source() {
    let s = store().await;
    let repo = s
        .repo_create("https://example.invalid/s.git", None)
        .await
        .unwrap();
    let stack = s
        .stack_create_git(
            LOCAL_HOST_ID,
            "blog",
            "Blog",
            repo.id,
            "refs/heads/main",
            "compose/blog.yml",
        )
        .await
        .unwrap();

    assert_eq!(stack.source_kind, SourceKind::Git);
    let git = stack.git.expect("git source");
    assert_eq!(git.repo_url, "https://example.invalid/s.git");
    assert_eq!(git.git_ref, "refs/heads/main");
    assert_eq!(git.compose_path, "compose/blog.yml");
    assert_eq!(git.last_commit, None);

    s.stack_set_last_commit(stack.id, "abc123").await.unwrap();
    let after = s.stack_by_id(stack.id).await.unwrap().unwrap();
    assert_eq!(after.git.unwrap().last_commit.as_deref(), Some("abc123"));
}

#[tokio::test]
async fn an_inline_stack_has_no_git_source() {
    let s = store().await;
    let stack = s
        .stack_create(LOCAL_HOST_ID, "local", "Local", "services: {}\n")
        .await
        .unwrap();

    assert_eq!(stack.source_kind, SourceKind::Inline);
    assert!(stack.git.is_none());
}

#[tokio::test]
async fn the_schema_refuses_a_half_specified_git_stack() {
    // A git stack is not addressable without all three parts, and an inline
    // stack has no business carrying any of them.
    let s = store().await;
    s.repo_create("https://example.invalid/s.git", None)
        .await
        .unwrap();

    let half = sqlx::query(
        "INSERT INTO stacks (host_id, slug, name, source_kind, compose_yaml,
                             repo_id, created_at, updated_at)
         VALUES (1, 'half', 'Half', 'git', '', 1, unixepoch(), unixepoch())",
    )
    .execute(s.pool())
    .await;
    assert!(
        half.is_err(),
        "a git stack without a ref or path must be refused"
    );

    let confused = sqlx::query(
        "INSERT INTO stacks (host_id, slug, name, source_kind, compose_yaml,
                             repo_id, git_ref, compose_path, created_at, updated_at)
         VALUES (1, 'confused', 'Confused', 'inline', 'services: {}', 1,
                 'refs/heads/main', 'x.yml', unixepoch(), unixepoch())",
    )
    .execute(s.pool())
    .await;
    assert!(
        confused.is_err(),
        "an inline stack must not carry a git source"
    );
}

#[tokio::test]
async fn environment_values_are_sealed_and_round_trip() {
    let s = store().await;
    let stack = s
        .stack_create(LOCAL_HOST_ID, "app", "App", "services: {}\n")
        .await
        .unwrap();

    let vars = vec![
        ("API_KEY".to_owned(), "sk_live_secret".to_owned()),
        ("TZ".to_owned(), "Europe/London".to_owned()),
    ];
    s.stack_env_set(stack.id, &vars).await.unwrap();

    let stored: String = sqlx::query("SELECT value_sealed FROM stack_env WHERE key = 'API_KEY'")
        .fetch_one(s.pool())
        .await
        .unwrap()
        .get(0);
    assert!(
        !stored.contains("sk_live_secret"),
        "value stored in the clear: {stored}"
    );

    assert_eq!(s.stack_env_get(stack.id).await.unwrap(), vars);
    assert_eq!(
        s.stack_env_keys(stack.id).await.unwrap(),
        vec!["API_KEY".to_owned(), "TZ".to_owned()],
        "keys are listable without revealing values"
    );
}

#[tokio::test]
async fn setting_the_environment_replaces_it_wholesale() {
    // A partial update would leave variables the user deleted still applied.
    let s = store().await;
    let stack = s
        .stack_create(LOCAL_HOST_ID, "app", "App", "services: {}\n")
        .await
        .unwrap();

    s.stack_env_set(stack.id, &[("OLD".to_owned(), "1".to_owned())])
        .await
        .unwrap();
    s.stack_env_set(stack.id, &[("NEW".to_owned(), "2".to_owned())])
        .await
        .unwrap();

    assert_eq!(
        s.stack_env_keys(stack.id).await.unwrap(),
        vec!["NEW".to_owned()]
    );
}

#[tokio::test]
async fn deleting_a_stack_takes_its_environment_with_it() {
    let s = store().await;
    let stack = s
        .stack_create(LOCAL_HOST_ID, "app", "App", "services: {}\n")
        .await
        .unwrap();
    s.stack_env_set(stack.id, &[("K".to_owned(), "v".to_owned())])
        .await
        .unwrap();

    s.stack_delete(stack.id).await.unwrap();

    assert!(s.stack_env_keys(stack.id).await.unwrap().is_empty());
}
