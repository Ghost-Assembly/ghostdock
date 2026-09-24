//! Against a real git repository.
//!
//! A local repository over a `file://` URL: genuinely git, no network, and
//! hermetic. Mocking git would only assert our own assumptions about it.

use std::path::Path;

use gitsync::{Error, Git, resolve_in_repo};

const COMPOSE: &str = "services:\n  web:\n    image: nginx:alpine\n";

fn run_git(args: &[&str], cwd: &Path) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_AUTHOR_NAME", "GhostDock tests")
        .env("GIT_AUTHOR_EMAIL", "tests@example.invalid")
        .env("GIT_COMMITTER_NAME", "GhostDock tests")
        .env("GIT_COMMITTER_EMAIL", "tests@example.invalid")
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A repository with one commit adding `compose/app.yml`.
fn origin(dir: &Path) -> String {
    run_git(&["init", "--quiet", "--initial-branch", "main", "."], dir);
    std::fs::create_dir_all(dir.join("compose")).unwrap();
    std::fs::write(dir.join("compose/app.yml"), COMPOSE).unwrap();
    run_git(&["add", "."], dir);
    run_git(&["commit", "--quiet", "-m", "add app"], dir);
    run_git(&["rev-parse", "HEAD"], dir)
}

#[tokio::test]
async fn reads_the_remote_head_without_cloning() {
    let remote = tempfile::tempdir().unwrap();
    let sha = origin(remote.path());
    let url = format!("file://{}", remote.path().display());

    let head = Git::new()
        .remote_head(&url, "refs/heads/main", None)
        .await
        .unwrap();

    assert_eq!(head, sha);
}

#[tokio::test]
async fn a_missing_ref_is_an_error_not_silence() {
    // Without --exit-code this succeeds with empty output, which would read
    // as "nothing changed" forever.
    let remote = tempfile::tempdir().unwrap();
    origin(remote.path());
    let url = format!("file://{}", remote.path().display());

    assert!(
        Git::new()
            .remote_head(&url, "refs/heads/does-not-exist", None)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn an_unreachable_remote_reports_gits_own_message() {
    let result = Git::new()
        .remote_head("file:///nonexistent/repo.git", "refs/heads/main", None)
        .await;

    match result {
        Err(Error::Git(message)) => assert!(
            !message.is_empty(),
            "the user needs git's explanation, not a generic failure"
        ),
        other => panic!("expected git's own error, got {other:?}"),
    }
}

#[tokio::test]
async fn syncs_a_working_tree_and_reads_a_file_from_it() {
    let remote = tempfile::tempdir().unwrap();
    let sha = origin(remote.path());
    let url = format!("file://{}", remote.path().display());
    let work = tempfile::tempdir().unwrap();
    let dir = work.path().join("repo");

    let git = Git::new();
    let synced = git.sync(&dir, &url, "refs/heads/main", None).await.unwrap();

    assert_eq!(synced, sha);
    assert_eq!(
        git.read_file(&dir, "compose/app.yml").await.unwrap(),
        COMPOSE
    );
}

#[tokio::test]
async fn syncing_again_picks_up_a_new_commit() {
    let remote = tempfile::tempdir().unwrap();
    origin(remote.path());
    let url = format!("file://{}", remote.path().display());
    let work = tempfile::tempdir().unwrap();
    let dir = work.path().join("repo");

    let git = Git::new();
    let first = git.sync(&dir, &url, "refs/heads/main", None).await.unwrap();

    std::fs::write(remote.path().join("compose/app.yml"), "services: {}\n").unwrap();
    run_git(&["commit", "--quiet", "-am", "change it"], remote.path());

    let second = git.sync(&dir, &url, "refs/heads/main", None).await.unwrap();

    assert_ne!(first, second);
    assert_eq!(
        Git::new().read_file(&dir, "compose/app.yml").await.unwrap(),
        "services: {}\n",
        "the working tree must reflect the new commit"
    );
}

#[tokio::test]
async fn local_edits_and_stray_files_are_discarded() {
    // The working tree is a cache of a commit. Anything left in it would
    // otherwise be deployed instead of what the repository says.
    let remote = tempfile::tempdir().unwrap();
    origin(remote.path());
    let url = format!("file://{}", remote.path().display());
    let work = tempfile::tempdir().unwrap();
    let dir = work.path().join("repo");

    let git = Git::new();
    git.sync(&dir, &url, "refs/heads/main", None).await.unwrap();

    std::fs::write(dir.join("compose/app.yml"), "tampered\n").unwrap();
    std::fs::write(dir.join("stray.txt"), "left behind\n").unwrap();

    git.sync(&dir, &url, "refs/heads/main", None).await.unwrap();

    assert_eq!(
        git.read_file(&dir, "compose/app.yml").await.unwrap(),
        COMPOSE
    );
    assert!(
        !dir.join("stray.txt").exists(),
        "untracked files must be cleaned"
    );
}

#[tokio::test]
async fn a_missing_file_says_so_plainly() {
    let remote = tempfile::tempdir().unwrap();
    origin(remote.path());
    let url = format!("file://{}", remote.path().display());
    let work = tempfile::tempdir().unwrap();
    let dir = work.path().join("repo");

    let git = Git::new();
    git.sync(&dir, &url, "refs/heads/main", None).await.unwrap();

    assert!(matches!(
        git.read_file(&dir, "compose/missing.yml").await,
        Err(Error::NotInRepo(_))
    ));
}

#[test]
fn a_compose_path_cannot_escape_the_repository() {
    let root = Path::new("/var/lib/ghostdock/stacks/blog/repo");

    for bad in [
        "../../../etc/passwd",
        "compose/../../escape.yml",
        "/etc/passwd",
        "..",
        "./..",
        "",
        ".",
    ] {
        assert!(
            resolve_in_repo(root, bad).is_err(),
            "{bad:?} must be refused"
        );
    }

    assert_eq!(
        resolve_in_repo(root, "compose/app.yml").unwrap(),
        root.join("compose/app.yml")
    );
    assert_eq!(
        resolve_in_repo(root, "./compose/app.yml").unwrap(),
        root.join("compose/app.yml")
    );
}

#[tokio::test]
async fn lists_the_tracked_files_of_a_checkout() {
    let remote = tempfile::tempdir().unwrap();
    origin(remote.path());
    std::fs::create_dir_all(remote.path().join("apps/wiki")).unwrap();
    std::fs::write(remote.path().join("apps/wiki/compose.yaml"), COMPOSE).unwrap();
    std::fs::write(remote.path().join("a name with spaces.yml"), COMPOSE).unwrap();
    run_git(&["add", "."], remote.path());
    run_git(&["commit", "--quiet", "-m", "more"], remote.path());
    let url = format!("file://{}", remote.path().display());

    let local = tempfile::tempdir().unwrap();
    let git = Git::new();
    git.sync(local.path(), &url, "refs/heads/main", None)
        .await
        .unwrap();
    // Untracked files in the checkout are not the repository's.
    std::fs::write(local.path().join("stray.yml"), COMPOSE).unwrap();

    let mut files = git.list_files(local.path()).await.unwrap();
    files.sort();
    assert_eq!(
        files,
        [
            "a name with spaces.yml",
            "apps/wiki/compose.yaml",
            "compose/app.yml"
        ]
    );
}
