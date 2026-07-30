use super::*;
use crate::core::instruction_paths::resolve_target;
use tempfile::TempDir;

fn fixture_session(temp: &TempDir) -> InstructionSession {
    let canonical = temp.path().canonicalize().expect("canonical tempdir");
    let root_source = load_source(
        &temp.path().join("AGENTS.md"),
        InstructionSourceKind::ProjectRoot,
        InstructionScope::ProjectRoot(canonical.display().to_string()),
        InstructionOwner::Main,
    );
    let root = Arc::new(InstructionRoot {
        workspace_cwd: canonical.clone(),
        project_root: canonical,
        base: root_source.into_iter().map(Arc::new).collect(),
        budget: Arc::new(Mutex::new(ConversationBudget::default())),
    });
    InstructionSession::new(root, InstructionOwner::Main)
}

#[test]
fn discovers_only_target_ancestor_chain() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir(temp.path().join(".git")).expect("git marker");
    fs::create_dir_all(temp.path().join("frontend/src")).expect("frontend");
    fs::create_dir_all(temp.path().join("backend")).expect("backend");
    fs::write(temp.path().join("AGENTS.md"), "root").expect("root rules");
    fs::write(temp.path().join("frontend/AGENTS.md"), "front").expect("front rules");
    fs::write(temp.path().join("backend/AGENTS.md"), "back").expect("back rules");

    let mut session = fixture_session(&temp);
    let target =
        resolve_target(session.root.workspace_cwd(), "frontend/src/lib.rs").expect("target");
    let discovery = session.discover(&[target]);

    assert!(discovery.activated);
    let rendered = session.rendered(false);
    assert!(rendered.system_prompt.contains("front"));
    assert!(!rendered.system_prompt.contains("back"));
    assert!(rendered.system_prompt.contains("kind=nested"));
    assert!(rendered.system_prompt.contains("scope="));
}

#[test]
fn cached_nested_source_does_not_change_between_submissions() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join("frontend")).expect("frontend");
    let source = temp.path().join("frontend/AGENTS.md");
    fs::write(&source, "first rules").expect("initial source");
    let mut session = fixture_session(&temp);
    let target = resolve_target(session.root.workspace_cwd(), "frontend/file.rs").expect("target");

    session.begin_submission();
    assert!(session.discover(std::slice::from_ref(&target)).activated);
    session.finish_submission();
    fs::write(&source, "second rules").expect("changed source");
    session.begin_submission();
    assert!(session.discover(&[target]).activated);

    let prompt = session.rendered(false).system_prompt;
    assert!(prompt.contains("first rules"));
    assert!(!prompt.contains("second rules"));
}

#[test]
fn restart_finishes_then_begins_with_no_duplicate_active_scope() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join("frontend")).expect("frontend");
    fs::write(temp.path().join("frontend/AGENTS.md"), "front rules").expect("nested rules");
    let mut session = fixture_session(&temp);
    let target = resolve_target(session.root.workspace_cwd(), "frontend/file.rs").expect("target");

    session.begin_submission();
    assert!(session.discover(std::slice::from_ref(&target)).activated);
    session.restart_submission();

    assert!(
        !session
            .rendered(false)
            .system_prompt
            .contains("front rules")
    );
    assert!(session.discover(&[target]).activated);
}

#[test]
fn blocked_nested_source_is_stable_for_one_owner() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join("frontend")).expect("frontend");
    let source = temp.path().join("frontend/AGENTS.md");
    fs::write(&source, [0xff]).expect("invalid source");
    let mut session = fixture_session(&temp);
    let target = resolve_target(session.root.workspace_cwd(), "frontend/file.rs").expect("target");

    assert!(
        session
            .discover(std::slice::from_ref(&target))
            .error
            .is_some()
    );
    fs::write(&source, "fixed rules").expect("fixed source");
    session.begin_submission();
    assert!(
        session
            .discover(std::slice::from_ref(&target))
            .error
            .is_some()
    );

    let mut new_owner = InstructionSession::new(session.root(), InstructionOwner::Main);
    assert!(new_owner.discover(&[target]).error.is_none());
    assert!(
        new_owner
            .rendered(false)
            .system_prompt
            .contains("fixed rules")
    );
}

#[test]
fn project_budget_is_bounded_and_prefers_the_nested_source() {
    let temp = TempDir::new().expect("tempdir");
    fs::create_dir_all(temp.path().join("frontend")).expect("frontend");
    fs::write(temp.path().join("AGENTS.md"), "r".repeat(20_000)).expect("root rules");
    fs::write(temp.path().join("frontend/AGENTS.md"), "n".repeat(20_000)).expect("nested rules");
    let mut session = fixture_session(&temp);
    let target = resolve_target(session.root.workspace_cwd(), "frontend/file.rs").expect("target");
    assert!(session.discover(&[target]).activated);

    let rendered = session.rendered(false);
    let project_bytes = rendered
        .system_prompt
        .len()
        .saturating_sub(BUILTIN_INSTRUCTIONS.len() + 2);
    assert!(project_bytes <= PROJECT_CHAIN_LIMIT);
    let root = rendered
        .summaries
        .iter()
        .find(|source| source.kind == InstructionSourceKind::ProjectRoot);
    let nested = rendered
        .summaries
        .iter()
        .find(|source| source.kind == InstructionSourceKind::Nested)
        .expect("nested summary");
    assert_eq!(nested.retained_bytes, 20_000);
    assert!(root.expect("root summary").truncated);
}
