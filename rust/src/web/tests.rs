use std::{
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
    sync::Mutex,
};

use async_trait::async_trait;

use crate::core::config::Config;

use super::repository::Stage;
use super::{
    prepare, sanitize_repo_url, CommandOutput, CommandRequest, CommandRunner, SystemCommandRunner,
    WebFailureKind,
};

#[derive(Debug)]
enum SideEffect {
    None,
    CreateCheckout(PathBuf),
    CreateIndex(PathBuf),
}

#[derive(Debug)]
struct ExpectedCommand {
    request: CommandRequest,
    output: CommandOutput,
    side_effect: SideEffect,
}

#[derive(Debug, Default)]
struct FakeRunner {
    expected: Mutex<VecDeque<ExpectedCommand>>,
}

impl FakeRunner {
    fn new(expected: Vec<ExpectedCommand>) -> Self {
        Self {
            expected: Mutex::new(expected.into()),
        }
    }

    fn assert_finished(&self) {
        let remaining = self.expected.lock().expect("runner lock");
        assert!(remaining.is_empty(), "unconsumed commands: {remaining:#?}");
    }
}

#[async_trait]
impl CommandRunner for FakeRunner {
    async fn run(&self, request: &CommandRequest) -> io::Result<CommandOutput> {
        let expected = self
            .expected
            .lock()
            .expect("runner lock")
            .pop_front()
            .expect("unexpected command");
        assert_eq!(&expected.request, request);
        match expected.side_effect {
            SideEffect::None => {}
            SideEffect::CreateCheckout(path) => {
                std::fs::create_dir_all(path.join(".git")).expect("create fake checkout");
            }
            SideEffect::CreateIndex(path) => {
                std::fs::create_dir_all(path.parent().expect("index parent")).expect("create dist");
                std::fs::write(path, "<!doctype html>").expect("write index");
            }
        }
        Ok(expected.output)
    }
}

fn ok(stdout: &str) -> CommandOutput {
    CommandOutput {
        status: Some(0),
        stdout: stdout.to_owned(),
        stderr: String::new(),
    }
}

fn expected(request: CommandRequest, stdout: &str) -> ExpectedCommand {
    ExpectedCommand {
        request,
        output: ok(stdout),
        side_effect: SideEffect::None,
    }
}

fn request(program: &str, args: &[&str], cwd: &Path) -> CommandRequest {
    CommandRequest::new(
        program,
        args.iter().map(|arg| (*arg).to_owned()).collect(),
        cwd.to_path_buf(),
    )
}

fn temporary_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("lqxp-web-{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("create temporary root");
    root
}

fn web_config(root: &Path, fresh: bool, tag: &str) -> Config {
    let mut config = Config::default();
    config.web.repo = "https://github.com/lqxp/client.git".to_owned();
    config.web.directory = "web".to_owned();
    config.web.fresh = fresh;
    config.web.tag = tag.to_owned();
    config.network.public_dir = root.join("web/dist").to_string_lossy().into_owned();
    config.network.webchat_index = "index.html".to_owned();
    config
}

#[tokio::test]
async fn command_runner_executes_without_shell() {
    let request = CommandRequest::new(
        "rustc",
        vec!["--version".to_owned()],
        PathBuf::from(env!("CARGO_MANIFEST_DIR")),
    );

    let output = SystemCommandRunner
        .run(&request)
        .await
        .expect("run rustc directly");

    assert!(output.success());
    assert!(output.stdout.starts_with("rustc "), "{}", output.stdout);
}

#[test]
fn authenticated_https_repo_is_redacted() {
    assert_eq!(
        sanitize_repo_url("https://alice:secret@github.com/lqxp/client.git"),
        "https://***@github.com/lqxp/client.git"
    );
    assert_eq!(
        sanitize_repo_url("git@github.com:lqxp/client.git"),
        "git@github.com:lqxp/client.git"
    );
}

#[tokio::test]
async fn fresh_checkout_builds_remote_head_before_returning() {
    let root = temporary_root("fresh");
    let checkout = root.join("web");
    let index = checkout.join("dist/index.html");
    let commit = "0123456789abcdef";
    let mut clone = expected(
        request(
            "git",
            &[
                "clone",
                "--no-checkout",
                "https://github.com/lqxp/client.git",
                checkout.to_str().unwrap(),
            ],
            &root,
        ),
        "",
    );
    clone.side_effect = SideEffect::CreateCheckout(checkout.clone());
    let mut build = expected(request("bun", &["run", "build"], &checkout), "");
    build.side_effect = SideEffect::CreateIndex(index.clone());
    let runner = FakeRunner::new(vec![
        expected(
            request("git", &["--version"], &root),
            "git version 2.51.0\n",
        ),
        expected(request("bun", &["--version"], &root), "1.2.22\n"),
        clone,
        expected(
            request(
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "remote",
                    "set-url",
                    "origin",
                    "https://github.com/lqxp/client.git",
                ],
                &root,
            ),
            "",
        ),
        expected(
            request(
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "fetch",
                    "--force",
                    "--prune",
                    "--tags",
                    "origin",
                ],
                &root,
            ),
            "",
        ),
        expected(
            request(
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "remote",
                    "set-head",
                    "origin",
                    "--auto",
                ],
                &root,
            ),
            "",
        ),
        expected(
            request(
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "rev-parse",
                    "--verify",
                    "refs/remotes/origin/HEAD^{commit}",
                ],
                &root,
            ),
            &format!("{commit}\n"),
        ),
        expected(
            request(
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "checkout",
                    "--detach",
                    "--force",
                    commit,
                ],
                &root,
            ),
            "",
        ),
        expected(
            request(
                "git",
                &["-C", checkout.to_str().unwrap(), "reset", "--hard", commit],
                &root,
            ),
            "",
        ),
        expected(
            request(
                "git",
                &["-C", checkout.to_str().unwrap(), "clean", "-fdx"],
                &root,
            ),
            "",
        ),
        expected(request("bun", &["install"], &checkout), ""),
        build,
    ]);

    let prepared = prepare(&web_config(&root, true, ""), &root, &runner)
        .await
        .expect("prepare fresh checkout");

    assert_eq!(prepared.revision, commit);
    assert_eq!(prepared.index, index);
    runner.assert_finished();
    std::fs::remove_dir_all(root).expect("remove temporary root");
}

#[tokio::test]
async fn tagged_checkout_validates_and_resolves_exact_tag() {
    let root = temporary_root("tagged");
    let checkout = root.join("web");
    let index = checkout.join("dist/index.html");
    let commit = "fedcba9876543210";
    let mut clone = expected(
        request(
            "git",
            &[
                "clone",
                "--no-checkout",
                "https://github.com/lqxp/client.git",
                checkout.to_str().unwrap(),
            ],
            &root,
        ),
        "",
    );
    clone.side_effect = SideEffect::CreateCheckout(checkout.clone());
    let mut build = expected(request("bun", &["run", "build"], &checkout), "");
    build.side_effect = SideEffect::CreateIndex(index.clone());
    let runner = FakeRunner::new(vec![
        expected(
            request("git", &["--version"], &root),
            "git version 2.51.0\n",
        ),
        expected(request("bun", &["--version"], &root), "1.2.22\n"),
        clone,
        expected(
            request(
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "remote",
                    "set-url",
                    "origin",
                    "https://github.com/lqxp/client.git",
                ],
                &root,
            ),
            "",
        ),
        expected(
            request(
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "fetch",
                    "--force",
                    "--prune",
                    "--tags",
                    "origin",
                ],
                &root,
            ),
            "",
        ),
        expected(
            request(
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "check-ref-format",
                    "refs/tags/v1.20.5",
                ],
                &root,
            ),
            "",
        ),
        expected(
            request(
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "rev-parse",
                    "--verify",
                    "refs/tags/v1.20.5^{commit}",
                ],
                &root,
            ),
            &format!("{commit}\n"),
        ),
        expected(
            request(
                "git",
                &[
                    "-C",
                    checkout.to_str().unwrap(),
                    "checkout",
                    "--detach",
                    "--force",
                    commit,
                ],
                &root,
            ),
            "",
        ),
        expected(
            request(
                "git",
                &["-C", checkout.to_str().unwrap(), "reset", "--hard", commit],
                &root,
            ),
            "",
        ),
        expected(
            request(
                "git",
                &["-C", checkout.to_str().unwrap(), "clean", "-fdx"],
                &root,
            ),
            "",
        ),
        expected(request("bun", &["install"], &checkout), ""),
        build,
    ]);

    let prepared = prepare(&web_config(&root, false, "v1.20.5"), &root, &runner)
        .await
        .expect("prepare tagged checkout");

    assert_eq!(prepared.revision, commit);
    assert_eq!(prepared.index, index);
    runner.assert_finished();
    std::fs::remove_dir_all(root).expect("remove temporary root");
}

#[derive(Debug)]
struct ScenarioState {
    fail_stage: Option<Stage>,
    failures_left: usize,
    failure_stderr: String,
    omit_built_index: bool,
    history: Vec<Stage>,
}

#[derive(Debug)]
struct ScenarioRunner {
    checkout: PathBuf,
    index: PathBuf,
    state: Mutex<ScenarioState>,
}

impl ScenarioRunner {
    fn stage_for(request: &CommandRequest) -> Stage {
        if request.program == "bun" {
            return match request.args.as_slice() {
                [arg] if arg == "--version" => Stage::ToolBun,
                [arg] if arg == "install" => Stage::BunInstall,
                [run, build] if run == "run" && build == "build" => Stage::BunBuild,
                other => panic!("unexpected Bun arguments: {other:?}"),
            };
        }
        match request.args.as_slice() {
            [arg] if arg == "--version" => Stage::ToolGit,
            [clone, no_checkout, ..] if clone == "clone" && no_checkout == "--no-checkout" => {
                Stage::Clone
            }
            [dash_c, _, command, rest @ ..] if dash_c == "-C" => match command.as_str() {
                "rev-parse"
                    if rest.first().map(String::as_str) == Some("--is-inside-work-tree") =>
                {
                    Stage::InspectCheckout
                }
                "rev-parse" => Stage::ResolveRevision,
                "remote" if rest.first().map(String::as_str) == Some("set-url") => Stage::SetOrigin,
                "remote" => Stage::ResolveRevision,
                "fetch" => Stage::Fetch,
                "check-ref-format" => Stage::ValidateTag,
                "checkout" => Stage::Checkout,
                "reset" => Stage::Reset,
                "clean" => Stage::Clean,
                other => panic!("unexpected Git command: {other}"),
            },
            other => panic!("unexpected Git arguments: {other:?}"),
        }
    }
}

#[async_trait]
impl CommandRunner for ScenarioRunner {
    async fn run(&self, request: &CommandRequest) -> io::Result<CommandOutput> {
        let stage = Self::stage_for(request);
        let mut state = self.state.lock().expect("scenario lock");
        state.history.push(stage);
        if state.fail_stage == Some(stage) && state.failures_left > 0 {
            if state.failures_left != usize::MAX {
                state.failures_left -= 1;
            }
            return Ok(CommandOutput {
                status: Some(1),
                stdout: String::new(),
                stderr: state.failure_stderr.clone(),
            });
        }
        match stage {
            Stage::Clone => {
                std::fs::create_dir_all(self.checkout.join(".git")).expect("create clone");
            }
            Stage::BunBuild if !state.omit_built_index => {
                std::fs::create_dir_all(self.index.parent().expect("index parent"))
                    .expect("create dist");
                std::fs::write(&self.index, "<!doctype html>").expect("write index");
            }
            _ => {}
        }
        let stdout = match stage {
            Stage::ResolveRevision if request.args.iter().any(|arg| arg == "rev-parse") => {
                "0123456789abcdef\n"
            }
            Stage::ToolGit => "git version 2.51.0\n",
            Stage::ToolBun => "1.2.22\n",
            Stage::InspectCheckout => "true\n",
            _ => "",
        };
        Ok(ok(stdout))
    }
}

#[derive(Debug)]
struct TestRig {
    root: PathBuf,
    config: Config,
    runner: ScenarioRunner,
}

impl TestRig {
    fn existing_checkout() -> Self {
        let root = temporary_root("recovery");
        let checkout = root.join("web");
        std::fs::create_dir_all(checkout.join(".git")).expect("create existing checkout");
        std::fs::write(checkout.join("marker"), "local state").expect("write marker");
        let index = checkout.join("dist/index.html");
        Self {
            config: web_config(&root, true, ""),
            runner: ScenarioRunner {
                checkout,
                index,
                state: Mutex::new(ScenarioState {
                    fail_stage: None,
                    failures_left: 0,
                    failure_stderr: String::new(),
                    omit_built_index: false,
                    history: Vec::new(),
                }),
            },
            root,
        }
    }

    fn checkout_file() -> Self {
        let root = temporary_root("checkout-file");
        let checkout = root.join("web");
        std::fs::write(&checkout, "ghost file").expect("create checkout file");
        let index = checkout.join("dist/index.html");
        Self {
            config: web_config(&root, true, ""),
            runner: ScenarioRunner {
                checkout,
                index,
                state: Mutex::new(ScenarioState {
                    fail_stage: None,
                    failures_left: 0,
                    failure_stderr: String::new(),
                    omit_built_index: false,
                    history: Vec::new(),
                }),
            },
            root,
        }
    }

    fn fail_once(&mut self, stage: Stage, stderr: &str) {
        let state = self.runner.state.get_mut().expect("scenario state");
        state.fail_stage = Some(stage);
        state.failures_left = 1;
        state.failure_stderr = stderr.to_owned();
    }

    fn fail_always(&mut self, stage: Stage, stderr: &str) {
        let state = self.runner.state.get_mut().expect("scenario state");
        state.fail_stage = Some(stage);
        state.failures_left = usize::MAX;
        state.failure_stderr = stderr.to_owned();
    }

    fn omit_built_index(&mut self) {
        self.runner
            .state
            .get_mut()
            .expect("scenario state")
            .omit_built_index = true;
    }

    async fn prepare(&self) -> Result<super::PreparedWeb, super::WebError> {
        prepare(&self.config, &self.root, &self.runner).await
    }

    fn marker_path(&self) -> PathBuf {
        self.root.join("web/marker")
    }

    fn stage_count(&self, stage: Stage) -> usize {
        self.runner
            .state
            .lock()
            .expect("scenario lock")
            .history
            .iter()
            .filter(|recorded| **recorded == stage)
            .count()
    }
}

impl Drop for TestRig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[tokio::test]
async fn local_checkout_corruption_deletes_once_and_reclones() {
    let mut rig = TestRig::existing_checkout();
    rig.fail_once(Stage::InspectCheckout, "fatal: bad object HEAD");
    let marker = rig.marker_path();

    let prepared = rig.prepare().await.expect("clean clone succeeds");

    assert!(prepared.index.is_file());
    assert!(!marker.exists());
    assert_eq!(rig.stage_count(Stage::InspectCheckout), 1);
    assert_eq!(rig.stage_count(Stage::Clone), 1);
}

#[tokio::test]
async fn checkout_file_is_removed_before_clean_clone() {
    let mut rig = TestRig::checkout_file();
    rig.fail_once(Stage::InspectCheckout, "fatal: not a git repository");

    let prepared = rig.prepare().await.expect("checkout file is recoverable");

    assert!(prepared.checkout.is_dir());
    assert!(prepared.index.is_file());
    assert_eq!(rig.stage_count(Stage::Clone), 1);
}

#[tokio::test]
async fn nested_git_repository_in_node_modules_is_removed_before_install() {
    let rig = TestRig::existing_checkout();
    let marker = rig.root.join("web/node_modules/package/.git/marker");
    std::fs::create_dir_all(marker.parent().expect("marker parent"))
        .expect("create nested repository");
    std::fs::write(&marker, "stale dependency").expect("write dependency marker");

    rig.prepare().await.expect("prepare existing checkout");

    assert!(!marker.exists());
}

#[tokio::test]
async fn second_local_failure_stops_after_one_reclone() {
    let mut rig = TestRig::existing_checkout();
    rig.fail_always(Stage::SetOrigin, "fatal: bad repository state");

    let error = rig.prepare().await.expect_err("second failure is fatal");

    assert_eq!(error.kind(), WebFailureKind::LocalCheckout);
    assert_eq!(rig.stage_count(Stage::SetOrigin), 2);
    assert_eq!(rig.stage_count(Stage::Clone), 1);
}

#[tokio::test]
async fn fetch_failure_preserves_existing_checkout() {
    let mut rig = TestRig::existing_checkout();
    rig.fail_once(
        Stage::Fetch,
        "fatal: unable to access: Could not resolve host",
    );
    let marker = rig.marker_path();

    let error = rig.prepare().await.expect_err("network error is fatal");

    assert_eq!(error.kind(), WebFailureKind::External);
    assert!(marker.exists());
    assert_eq!(rig.stage_count(Stage::Clone), 0);
}

#[tokio::test]
async fn stale_git_lock_during_fetch_deletes_once_and_reclones() {
    let mut rig = TestRig::existing_checkout();
    rig.fail_once(
        Stage::Fetch,
        "fatal: Unable to create '/srv/lqxp/web/.git/shallow.lock': File exists.",
    );
    let marker = rig.marker_path();

    let prepared = rig.prepare().await.expect("stale Git lock is recoverable");

    assert!(prepared.index.is_file());
    assert!(!marker.exists());
    assert_eq!(rig.stage_count(Stage::Fetch), 2);
    assert_eq!(rig.stage_count(Stage::Clone), 1);
}

#[tokio::test]
async fn permission_failure_preserves_existing_checkout() {
    let mut rig = TestRig::existing_checkout();
    rig.fail_once(Stage::Clean, "fatal: cannot unlink: Permission denied");
    let marker = rig.marker_path();

    let error = rig.prepare().await.expect_err("permission error is fatal");

    assert_eq!(error.kind(), WebFailureKind::System);
    assert!(marker.exists());
    assert_eq!(rig.stage_count(Stage::Clone), 0);
}

#[tokio::test]
async fn missing_bun_stops_before_repository_mutation() {
    let mut rig = TestRig::existing_checkout();
    rig.fail_once(Stage::ToolBun, "No such file or directory");
    let marker = rig.marker_path();

    let error = rig.prepare().await.expect_err("missing Bun is fatal");

    assert_eq!(error.kind(), WebFailureKind::System);
    assert!(marker.exists());
    assert_eq!(rig.stage_count(Stage::InspectCheckout), 0);
}

#[tokio::test]
async fn missing_built_index_is_fatal_after_successful_commands() {
    let mut rig = TestRig::existing_checkout();
    rig.omit_built_index();

    let error = rig.prepare().await.expect_err("index is required");

    assert_eq!(error.kind(), WebFailureKind::Output);
    assert_eq!(rig.stage_count(Stage::BunBuild), 1);
    assert_eq!(rig.stage_count(Stage::Clone), 0);
}
