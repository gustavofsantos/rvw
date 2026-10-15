//! The MCP handler: one tool per `Service` operation. Port of
//! internal/mcpserver/server.go. A tool's input and output schemas are the
//! operation types' own JSON Schemas, so the schemas an agent sees are the ones
//! in `rvw_core::operations`.

use std::future::Future;
use std::sync::Arc;

use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
    Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use rvw_core::Service;
use rvw_core::operations::{
    AddInput, EditInput, GateInput, GateOutput, GetInput, OverrideInput, PullInput, QueryInput,
    RemoveInput, ReportSessionInput, ReportSessionOutput, ResolveInput, SheetsInput, SubmitInput,
    VerdictInput, VerdictOutput, WorkspacesInput,
};
use rvw_core::types::{Actor, GateOverride};
use schemars::JsonSchema;
use schemars::generate::SchemaSettings;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::Options;
use crate::scope::scope;

/// JSON object, the shape of tool arguments and schemas.
type Object = Map<String, Value>;

/// What the server tells every client about how to use it.
const INSTRUCTIONS: &str = r#"rvw is a local queue of code review comments, keyed by workspace (the git
root of a directory; a directory outside git has none). Pass your project
directory as "workspace" on every call.

To address review comments: call pull once (pulled comments leave the queue;
use peek to look without taking). Work both the standalone "comments" and the
comments inside each of "reviews"; read each review's summary as context. Then
call resolve on every comment id (r3, not rv1) right after you finish it:
outcome "done" with a note of what you did, or "rejected" with the reason.
Never leave a pulled comment undecided; list with status "pulled" shows them.
A comment with `reopens` is a follow-up: the reviewer rejected your earlier answer, read their note. `points_at` names another file range the comment refers to.

To review code for another agent: add comments on line ranges, then submit
them as one review with a decision (comment, approve, request-changes).
Only a reviewing agent judges, and never its own decision.

To record the session behind your commits: call report_session with your
session id, tool, model and the commit ids you made. Call it again as you
commit; a repeat adds commits, and the first report's tool and model stay.

Pass "author" with your name on writes. A pull pinned to a lane takes only
that lane; "elsewhere" says when work waits in other lanes."#;

/// State shared by every session of one server.
struct Shared {
    svc: Arc<Service>,
    opts: Options,
    tools: Vec<Tool>,
}

/// One MCP session's handler. Cheap to clone: it shares [`Shared`].
#[derive(Clone)]
pub struct RvwServer(Arc<Shared>);

impl RvwServer {
    pub fn new(svc: Arc<Service>, opts: Options) -> Self {
        let mut tools = tool_definitions();
        // The Go server lists tools by name; clients may depend on that order.
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        Self(Arc::new(Shared { svc, opts, tools }))
    }
}

impl ServerHandler for RvwServer {
    fn get_info(&self) -> ServerConfig {
        let mut info = InitializeResult::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        );
        info.server_info = Implementation::new("rvw", self.0.opts.version.clone());
        info.server_info.title = Some("rvw review queue".to_owned());
        info.instructions = Some(INSTRUCTIONS.to_owned());
        info
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(self.0.tools.clone())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let name = request.name.to_string();
        if !self.0.tools.iter().any(|t| t.name == name) {
            return Err(ErrorData::invalid_params(
                format!("unknown tool \"{name}\""),
                None,
            ));
        }
        let args = request.arguments.unwrap_or_default();
        let shared = Arc::clone(&self.0);
        // The service blocks on SQLite and git, so it runs off the async workers.
        let outcome = tokio::task::spawn_blocking(move || invoke(&shared, &name, args))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(match outcome {
            Ok(value) => CallToolResult::structured(value).into(),
            Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]).into(),
        })
    }
}

/// Runs one tool call: validates the arguments against the tool's input
/// schema, then the operation. Both kinds of failure come back as the text of
/// a tool error result.
fn invoke(shared: &Shared, name: &str, args: Object) -> Result<Value, String> {
    let tool = shared
        .tools
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| format!("unknown tool \"{name}\""))?;
    check_arguments(&tool.input_schema, &args)?;
    let ctx = Ctx(shared);
    match name {
        "add" => ctx.add(args),
        "submit" => ctx.submit(args),
        "list" => ctx.list(args),
        "list_reviews" => ctx.list_reviews(args),
        "count" => ctx.count(args),
        "pull" => ctx.pull(args),
        "show_comment" => ctx.show_comment(args),
        "show_review" => ctx.show_review(args),
        "resolve" => ctx.resolve(args),
        "edit" => ctx.edit(args),
        "remove" => ctx.remove(args),
        "report_session" => ctx.report_session(args),
        "judge" => ctx.judge(args),
        "gate" => ctx.gate(args),
        "override_gate" => ctx.override_gate(args),
        "workspaces" => ctx.workspaces(args),
        _ => Err(format!("unknown tool \"{name}\"")),
    }
}

/// The operations, one method per tool. Each parses its arguments, scopes the
/// fields the tool takes, and calls the service.
struct Ctx<'a>(&'a Shared);

impl Ctx<'_> {
    fn scope(
        &self,
        workspace: &mut String,
        lane: Option<&mut String>,
        author: Option<&mut String>,
    ) -> Result<(), String> {
        scope(&self.0.opts, workspace, lane, author)
    }

    fn add(&self, args: Object) -> Result<Value, String> {
        let mut input: AddInput = parse(args)?;
        self.scope(
            &mut input.workspace,
            Some(&mut input.lane),
            Some(&mut input.author),
        )?;
        reply(self.0.svc.add(input))
    }

    fn submit(&self, args: Object) -> Result<Value, String> {
        let mut input: SubmitInput = parse(args)?;
        self.scope(
            &mut input.workspace,
            Some(&mut input.lane),
            Some(&mut input.author),
        )?;
        reply(self.0.svc.submit(input))
    }

    fn list(&self, args: Object) -> Result<Value, String> {
        let mut input: QueryInput = parse(args)?;
        self.scope(&mut input.workspace, Some(&mut input.lane), None)?;
        reply(self.0.svc.list(input))
    }

    fn list_reviews(&self, args: Object) -> Result<Value, String> {
        let mut input: SheetsInput = parse(args)?;
        self.scope(&mut input.workspace, Some(&mut input.lane), None)?;
        reply(self.0.svc.sheets(input))
    }

    fn count(&self, args: Object) -> Result<Value, String> {
        let mut input: QueryInput = parse(args)?;
        self.scope(&mut input.workspace, Some(&mut input.lane), None)?;
        reply(self.0.svc.count(input))
    }

    fn pull(&self, args: Object) -> Result<Value, String> {
        let mut input: PullInput = parse(args)?;
        self.scope(&mut input.workspace, Some(&mut input.lane), None)?;
        reply(self.0.svc.pull(input))
    }

    fn show_comment(&self, args: Object) -> Result<Value, String> {
        let mut input: GetInput = parse(args)?;
        self.scope(&mut input.workspace, None, None)?;
        reply(self.0.svc.evidence(input))
    }

    fn show_review(&self, args: Object) -> Result<Value, String> {
        let mut input: GetInput = parse(args)?;
        self.scope(&mut input.workspace, None, None)?;
        reply(self.0.svc.sheet(input))
    }

    fn resolve(&self, args: Object) -> Result<Value, String> {
        let mut input: ResolveInput = parse(args)?;
        self.scope(&mut input.workspace, None, Some(&mut input.author))?;
        reply(self.0.svc.resolve(input))
    }

    fn edit(&self, args: Object) -> Result<Value, String> {
        let mut input: EditInput = parse(args)?;
        self.scope(&mut input.workspace, None, None)?;
        reply(self.0.svc.edit(input))
    }

    fn remove(&self, args: Object) -> Result<Value, String> {
        let mut input: RemoveInput = parse(args)?;
        self.scope(&mut input.workspace, None, None)?;
        reply(self.0.svc.remove(input))
    }

    /// Judges as the agent surface: the actor is never an argument.
    fn judge(&self, args: Object) -> Result<Value, String> {
        let mut input: VerdictInput = parse(args)?;
        self.scope(&mut input.workspace, None, Some(&mut input.author))?;
        reply(self.0.svc.judge(input, Actor::Agent))
    }

    fn gate(&self, args: Object) -> Result<Value, String> {
        let mut input: GateInput = parse(args)?;
        self.scope(&mut input.workspace, Some(&mut input.lane), None)?;
        reply(self.0.svc.gate(input))
    }

    /// Overrides as the agent surface: the actor is never an argument.
    fn override_gate(&self, args: Object) -> Result<Value, String> {
        let mut input: OverrideInput = parse(args)?;
        self.scope(
            &mut input.workspace,
            Some(&mut input.lane),
            Some(&mut input.author),
        )?;
        reply(self.0.svc.override_gate(input, Actor::Agent))
    }

    fn report_session(&self, args: Object) -> Result<Value, String> {
        let mut input: ReportSessionInput = parse(args)?;
        self.scope(&mut input.workspace, None, None)?;
        reply(self.0.svc.report_session(input))
    }

    fn workspaces(&self, args: Object) -> Result<Value, String> {
        let input: WorkspacesInput = parse(args)?;
        reply(self.0.svc.workspaces(input))
    }
}

/// Deserializes validated arguments into an operation's input.
fn parse<T: DeserializeOwned>(args: Object) -> Result<T, String> {
    serde_json::from_value(Value::Object(args))
        .map_err(|e| format!("validating \"arguments\": {e}"))
}

/// An operation's result as the structured output, or its error message.
fn reply<T: Serialize>(result: rvw_core::Result<T>) -> Result<Value, String> {
    match result {
        Ok(value) => serde_json::to_value(value).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Checks arguments against a tool's input schema the way the Go server's SDK
/// did: every required property present, and no property the schema does not
/// name. Type errors are left to deserialization.
fn check_arguments(schema: &Object, args: &Object) -> Result<(), String> {
    let missing: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|name| !args.contains_key(*name))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "validating \"arguments\": validating root: required: missing properties: {}",
            name_list(&missing)
        ));
    }
    let known = schema.get("properties").and_then(Value::as_object);
    let extra: Vec<&str> = args
        .keys()
        .map(String::as_str)
        .filter(|name| known.is_none_or(|props| !props.contains_key(*name)))
        .collect();
    if !extra.is_empty() {
        return Err(format!(
            "validating \"arguments\": validating root: unexpected additional properties {}",
            name_list(&extra)
        ));
    }
    Ok(())
}

fn name_list(names: &[&str]) -> String {
    Value::Array(names.iter().map(|n| Value::from(*n)).collect()).to_string()
}

/// The JSON Schema of `T` as a tool schema: the root is an object that takes no
/// unnamed properties, with no `$schema`, `title` or description of its own.
fn object_schema<T: JsonSchema>() -> Object {
    let root = SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<T>();
    let Ok(Value::Object(mut schema)) = serde_json::to_value(root) else {
        unreachable!("a struct's schema is a JSON object");
    };
    schema.remove("$schema");
    schema.remove("title");
    schema.remove("description");
    schema.insert("additionalProperties".to_owned(), Value::Bool(false));
    schema
}

fn tool<I: JsonSchema, O: JsonSchema>(
    name: &'static str,
    description: &'static str,
    read_only: bool,
) -> Tool {
    let mut tool = Tool::new(name, description, object_schema::<I>())
        .with_raw_output_schema(Arc::new(object_schema::<O>()));
    if read_only {
        tool = tool.with_annotations(ToolAnnotations::new().read_only(true));
    }
    tool
}

/// The tools in the order the Go server lists them.
fn tool_definitions() -> Vec<Tool> {
    use rvw_core::operations::{
        CountOutput, Evidence, ListOutput, PullOutput, ReviewSheet, SheetsOutput, WorkspacesOutput,
    };

    use rvw_core::types::{Comment, Review};
    vec![
        tool::<AddInput, Comment>(
            "add",
            "Enqueue one review comment on a line range of one file. The code is snapshotted from disk, or from source when given.",
            false,
        ),
        tool::<SubmitInput, Review>(
            "submit",
            "Submit one review with a decision and summary over pending comments. Without comment_ids it takes this author's pending, unlinked comments in this exact lane.",
            false,
        ),
        tool::<QueryInput, ListOutput>(
            "list",
            "List review comments, oldest first, without dequeuing them.",
            true,
        ),
        tool::<SheetsInput, SheetsOutput>(
            "list_reviews",
            "List submitted review sheets with their state (pending, pulled, complete), oldest first.",
            true,
        ),
        tool::<QueryInput, CountOutput>(
            "count",
            "Count pending handoffs (a submitted review counts once), or comments for any other status.",
            true,
        ),
        tool::<PullInput, PullOutput>(
            "pull",
            "Dequeue submitted reviews and standalone comments to act on. Pulled handoffs leave the queue; peek reads without dequeuing. Record a decision on every pulled comment with resolve.",
            false,
        ),
        tool::<GetInput, Evidence>(
            "show_comment",
            "Show one comment (r3) with its decision and, once done, the diff it produced.",
            true,
        ),
        tool::<GetInput, ReviewSheet>(
            "show_review",
            "Show one submitted review sheet (rv1) with its linked comments and state.",
            true,
        ),
        tool::<ResolveInput, Comment>(
            "resolve",
            "Record the decision on a comment: done (what you did; the comment must be pulled) or rejected (why not; a note is required). A decision is final.",
            false,
        ),
        tool::<EditInput, Comment>(
            "edit",
            "Replace the text of a pending comment; a pulled or decided one is refused. File, range and code snapshot are unchanged.",
            false,
        ),
        tool::<RemoveInput, Comment>(
            "remove",
            "Delete a pending comment that is not part of a submitted review, to retract a note from a draft. Returns the comment as it was; a pulled, decided or submitted one is refused.",
            false,
        ),
        tool::<ReportSessionInput, ReportSessionOutput>(
            "report_session",
            "Reports the agent session that did the work and the commits it made, so history can show which commits an agent made. Call it again as you commit; a repeat adds commits. The tool and model of the first report stay.",
            false,
        ),
        tool::<VerdictInput, VerdictOutput>(
            "judge",
            "Judges a decided comment: accept the decision, or reopen it with a note, which creates a follow-up comment in the same lane.",
            false,
        ),
        tool::<GateInput, GateOutput>(
            "gate",
            "Lists the open and unjudged comment ids that stand between a lane and a commit or push.",
            true,
        ),
        tool::<OverrideInput, GateOverride>(
            "override_gate",
            "Records going ahead with a commit or push although the gate is not clear, with the reason.",
            false,
        ),
        tool::<WorkspacesInput, WorkspacesOutput>(
            "workspaces",
            "List every workspace in the store with its pending handoff count.",
            true,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use rvw_core::Service;
    use serde_json::json;

    use super::*;

    fn service(dir: &std::path::Path) -> Arc<Service> {
        let store = rvw_store::Store::open(&dir.join("q.db")).expect("open store");
        Arc::new(Service::new(Box::new(store)))
    }

    fn args(value: &Value) -> Object {
        value.as_object().expect("object").clone()
    }

    /// A git repository with one file, and the canonical path to it.
    fn repo(dir: &std::path::Path) -> String {
        let root = dir.join("proj");
        fs::create_dir(&root).expect("mkdir");
        fs::write(root.join("app.py"), "one\ntwo\nthree\n").expect("write");
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(["init", "-q"])
                .status()
                .expect("git init")
                .success()
        );
        fs::canonicalize(&root)
            .expect("canonicalize")
            .to_string_lossy()
            .into_owned()
    }

    fn server(dir: &std::path::Path, lane: &str, author: &str) -> RvwServer {
        RvwServer::new(
            service(dir),
            Options {
                workspace: String::new(),
                lane: lane.to_owned(),
                author: author.to_owned(),
                version: "test".to_owned(),
            },
        )
    }

    /// A server over a shared service, pinned to `lane` and speaking as `author`,
    /// so several servers can see one queue.
    fn serve(svc: &Arc<Service>, lane: &str, author: &str) -> RvwServer {
        RvwServer::new(
            Arc::clone(svc),
            Options {
                workspace: String::new(),
                lane: lane.to_owned(),
                author: author.to_owned(),
                version: "test".to_owned(),
            },
        )
    }

    #[test]
    fn registers_the_sixteen_tools_in_name_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let names: Vec<String> = server(dir.path(), "", "")
            .0
            .tools
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        assert_eq!(
            names,
            [
                "add",
                "count",
                "edit",
                "gate",
                "judge",
                "list",
                "list_reviews",
                "override_gate",
                "pull",
                "remove",
                "report_session",
                "resolve",
                "show_comment",
                "show_review",
                "submit",
                "workspaces"
            ]
        );
    }

    #[test]
    fn only_gate_is_read_only_among_the_new_tools() {
        let dir = tempfile::tempdir().expect("tempdir");
        let srv = server(dir.path(), "", "");
        let read_only = |name: &str| {
            let tool = srv.0.tools.iter().find(|t| t.name == name).expect(name);
            tool.annotations.as_ref().and_then(|a| a.read_only_hint)
        };
        assert_eq!(read_only("gate"), Some(true));
        assert_ne!(read_only("report_session"), Some(true));
        assert_ne!(read_only("judge"), Some(true));
        assert_ne!(read_only("override_gate"), Some(true));
    }

    #[test]
    fn the_new_tools_take_no_actor_argument() {
        let dir = tempfile::tempdir().expect("tempdir");
        let srv = server(dir.path(), "", "");
        let required = |name: &str| {
            let tool = srv.0.tools.iter().find(|t| t.name == name).expect(name);
            assert!(
                !tool.input_schema["properties"]
                    .as_object()
                    .expect("properties")
                    .contains_key("actor"),
                "{name} takes an actor"
            );
            tool.input_schema["required"].clone()
        };
        assert_eq!(
            required("report_session"),
            json!(["workspace", "session_id", "tool", "model"])
        );
        assert_eq!(required("judge"), json!(["workspace", "id", "verdict"]));
        assert_eq!(required("gate"), json!(["workspace"]));
        assert_eq!(
            required("override_gate"),
            json!(["workspace", "action", "reason"])
        );
    }

    #[test]
    fn an_actor_argument_is_refused_like_any_unknown_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let srv = server(dir.path(), "", "");
        let cases = [
            (
                "judge",
                json!({"workspace": "/x", "id": "r1", "verdict": "accepted", "actor": "human"}),
            ),
            ("gate", json!({"workspace": "/x", "actor": "human"})),
            (
                "override_gate",
                json!({"workspace": "/x", "action": "commit", "reason": "why", "actor": "human"}),
            ),
        ];
        for (tool, arguments) in cases {
            let err = invoke(&srv.0, tool, args(&arguments)).unwrap_err();
            assert_eq!(
                err,
                r#"validating "arguments": validating root: unexpected additional properties ["actor"]"#,
                "{tool}"
            );
        }
    }

    #[test]
    fn the_instructions_teach_follow_ups_points_at_and_judging() {
        let dir = tempfile::tempdir().expect("tempdir");
        let text = server(dir.path(), "", "")
            .get_info()
            .instructions
            .expect("instructions");
        assert!(
            text.contains(
                "A comment with `reopens` is a follow-up: the reviewer rejected your earlier answer, read their note. `points_at` names another file range the comment refers to."
            ),
            "{text}"
        );
        assert!(
            text.contains("Only a reviewing agent judges, and never its own decision."),
            "{text}"
        );
        assert!(text.contains("call report_session with your"), "{text}");
    }

    #[test]
    fn judge_via_mcp_records_an_agent_verdict_and_the_follow_up_stays_in_the_lane() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = repo(dir.path());
        let svc = service(dir.path());
        // alice raises a comment in lane auth; bob pulls it and rejects it.
        let alice = serve(&svc, "auth", "alice");
        invoke(
            &alice.0,
            "add",
            args(
                &json!({"workspace": root, "file": "app.py", "start_line": 2, "comment": "rename"}),
            ),
        )
        .expect("add");
        let bob = serve(&svc, "auth", "bob");
        invoke(&bob.0, "pull", args(&json!({"workspace": root}))).expect("pull");
        invoke(
            &bob.0,
            "resolve",
            args(&json!({"workspace": root, "id": "r1", "outcome": "rejected", "note": "not needed"})),
        )
        .expect("resolve");

        // claude, pinned to no lane, judges it: the pinned author is the judge.
        let claude = serve(&svc, "", "claude");
        let judged = invoke(
            &claude.0,
            "judge",
            args(&json!({"workspace": root, "id": "r1", "verdict": "reopened", "note": "do it anyway"})),
        )
        .expect("judge");
        assert_eq!(judged["comment"]["id"], "r1");
        assert_eq!(judged["comment"]["verdict"]["kind"], "reopened");
        assert_eq!(judged["comment"]["verdict"]["by"], "claude");
        assert_eq!(judged["comment"]["verdict"]["actor"], "agent");
        assert_eq!(judged["comment"]["verdict"]["followup"], "r2");
        let follow_up = &judged["followup"];
        assert_eq!(follow_up["id"], "r2");
        assert_eq!(follow_up["reopens"], "r1");
        assert_eq!(follow_up["status"], "pending");
        assert_eq!(follow_up["lane"], "auth");
        assert_eq!(follow_up["comment"], "do it anyway");

        // A verdict is final.
        let err = invoke(
            &claude.0,
            "judge",
            args(&json!({"workspace": root, "id": "r1", "verdict": "accepted"})),
        )
        .unwrap_err();
        assert!(err.contains("already has a verdict"), "{err}");
    }

    #[test]
    fn a_lane_pinned_gate_sees_only_its_lane() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = repo(dir.path());
        let svc = service(dir.path());
        let unpinned = serve(&svc, "", "alice");
        invoke(
            &unpinned.0,
            "add",
            args(&json!({"workspace": root, "file": "app.py", "start_line": 1, "comment": "unlaned"})),
        )
        .expect("add");
        let auth = serve(&svc, "auth", "alice");
        invoke(
            &auth.0,
            "add",
            args(&json!({"workspace": root, "file": "app.py", "start_line": 2, "comment": "in auth"})),
        )
        .expect("add");

        let pinned = invoke(&auth.0, "gate", args(&json!({"workspace": root}))).expect("gate");
        assert_eq!(pinned["lane"], "auth");
        assert_eq!(pinned["open"], json!(["r2"]));
        assert_eq!(pinned["unjudged"], json!([]));
        assert_eq!(pinned["clear"], false);

        let everything =
            invoke(&unpinned.0, "gate", args(&json!({"workspace": root}))).expect("gate");
        assert_eq!(everything["lane"], Value::Null);
        assert_eq!(everything["open"], json!(["r1", "r2"]));

        let payments = serve(&svc, "payments", "alice");
        let clear = invoke(&payments.0, "gate", args(&json!({"workspace": root}))).expect("gate");
        assert_eq!(clear["open"], json!([]));
        assert_eq!(clear["clear"], true);
    }

    #[test]
    fn override_gate_via_mcp_records_an_agent_override() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = repo(dir.path());
        let svc = service(dir.path());
        // alice raises r1; bob pulls it and marks it done, so the gate is unjudged on it.
        let alice = serve(&svc, "", "alice");
        invoke(
            &alice.0,
            "add",
            args(&json!({"workspace": root, "file": "app.py", "start_line": 1, "comment": "one"})),
        )
        .expect("add");
        let bob = serve(&svc, "", "bob");
        invoke(&bob.0, "pull", args(&json!({"workspace": root}))).expect("pull");
        invoke(
            &bob.0,
            "resolve",
            args(&json!({"workspace": root, "id": "r1", "outcome": "done", "note": "fixed"})),
        )
        .expect("resolve");

        let gustav = serve(&svc, "", "gustav");
        let recorded = invoke(
            &gustav.0,
            "override_gate",
            args(&json!({"workspace": root, "action": "commit", "reason": "shipping the spike"})),
        )
        .expect("override_gate");
        assert_eq!(recorded["workspace"], root);
        assert_eq!(recorded["lane"], Value::Null);
        assert_eq!(recorded["action"], "commit");
        assert_eq!(recorded["reason"], "shipping the spike");
        assert_eq!(recorded["by"], "gustav");
        assert_eq!(recorded["actor"], "agent");
        assert_eq!(recorded["open"], json!([]));
        assert_eq!(recorded["unjudged"], json!(["r1"]));
    }

    #[test]
    fn override_gate_via_mcp_refuses_a_clear_gate_and_bad_input() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = repo(dir.path());
        let srv = server(dir.path(), "", "gustav");
        let err = invoke(
            &srv.0,
            "override_gate",
            args(&json!({"workspace": root, "action": "push", "reason": "why"})),
        )
        .unwrap_err();
        assert_eq!(err, "nothing is open or unjudged — no override needed");

        let err = invoke(
            &srv.0,
            "override_gate",
            args(&json!({"workspace": root, "action": "merge", "reason": "why"})),
        )
        .unwrap_err();
        assert_eq!(err, r#"action "merge" is not one of commit, push"#);
    }

    #[test]
    fn required_inputs_come_from_the_operation_types() {
        let dir = tempfile::tempdir().expect("tempdir");
        let srv = server(dir.path(), "", "");
        let add = srv.0.tools.iter().find(|t| t.name == "add").expect("add");
        assert_eq!(
            add.input_schema["required"],
            json!(["workspace", "file", "start_line", "comment"])
        );
        let list = srv.0.tools.iter().find(|t| t.name == "list").expect("list");
        assert_eq!(
            list.annotations.as_ref().and_then(|a| a.read_only_hint),
            Some(true)
        );
    }

    #[test]
    fn a_missing_required_argument_is_a_validation_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let srv = server(dir.path(), "", "");
        let err = invoke(&srv.0, "count", args(&json!({}))).unwrap_err();
        assert_eq!(
            err,
            r#"validating "arguments": validating root: required: missing properties: ["workspace"]"#
        );
    }

    #[test]
    fn an_unknown_argument_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let srv = server(dir.path(), "", "");
        let err = invoke(
            &srv.0,
            "count",
            args(&json!({"workspace": "/x", "bogus": 1})),
        )
        .unwrap_err();
        assert!(
            err.contains("unexpected additional properties [\"bogus\"]"),
            "{err}"
        );
    }

    #[test]
    fn add_pull_resolve_flows_through_the_queue() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = repo(dir.path());
        // The server pins lane "auth" and speaks as "claude".
        let srv = server(dir.path(), "auth", "claude");

        let added = invoke(
            &srv.0,
            "add",
            args(&json!({"workspace": root, "file": "app.py", "start_line": 2, "comment": "over mcp"})),
        )
        .expect("add");
        assert_eq!(added["id"], "r1");
        assert_eq!(added["lane"], "auth");
        assert_eq!(added["author"], "claude");
        assert_eq!(added["code"], "two");

        let pulled = invoke(
            &srv.0,
            "pull",
            args(&json!({"workspace": root, "lane": ""})),
        )
        .expect("pull");
        assert_eq!(pulled["drained"], true);
        assert_eq!(pulled["comments"][0]["id"], "r1");
        assert_eq!(pulled["comments"][0]["status"], "pulled");

        let resolved = invoke(
            &srv.0,
            "resolve",
            args(&json!({"workspace": root, "id": "r1", "outcome": "done", "note": "fixed"})),
        )
        .expect("resolve");
        assert_eq!(resolved["status"], "done");
        assert_eq!(resolved["resolved_by"], "claude");

        let pending = invoke(&srv.0, "count", args(&json!({"workspace": root}))).expect("count");
        assert_eq!(pending["count"], 0);
    }

    #[test]
    fn remove_retracts_a_draft_comment() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = repo(dir.path());
        let srv = server(dir.path(), "", "");
        invoke(
            &srv.0,
            "add",
            args(&json!({"workspace": root, "file": "app.py", "start_line": 1, "comment": "oops"})),
        )
        .expect("add");
        let gone = invoke(
            &srv.0,
            "remove",
            args(&json!({"workspace": root, "id": "r1"})),
        )
        .expect("remove");
        assert_eq!(gone["id"], "r1");
        assert_eq!(gone["comment"], "oops");
        let count = invoke(&srv.0, "count", args(&json!({"workspace": root}))).expect("count");
        assert_eq!(count["count"], 0);
        let err = invoke(
            &srv.0,
            "remove",
            args(&json!({"workspace": root, "id": "r1"})),
        )
        .unwrap_err();
        assert!(err.contains("r1"), "{err}");
    }

    #[test]
    fn a_service_failure_is_its_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = repo(dir.path());
        let srv = server(dir.path(), "", "");
        let err = invoke(
            &srv.0,
            "resolve",
            args(&json!({"workspace": root, "id": "r9", "outcome": "done"})),
        )
        .unwrap_err();
        assert!(err.contains("r9"), "{err}");
    }

    #[test]
    fn a_call_outside_git_is_a_scope_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let plain = fs::canonicalize(dir.path()).expect("canonicalize");
        let srv = server(dir.path(), "", "");
        let err = invoke(
            &srv.0,
            "count",
            args(&json!({"workspace": plain.to_string_lossy()})),
        )
        .unwrap_err();
        assert!(err.contains("is not inside a git repository"), "{err}");
    }

    /// Calls a tool and checks that its result satisfies the tool's own output
    /// schema, as a strict client (Cursor) does before it hands the result on.
    fn checked(srv: &RvwServer, name: &str, arguments: Value) -> Value {
        let Value::Object(arguments) = arguments else {
            panic!("tool arguments are a JSON object");
        };
        let value =
            invoke(&srv.0, name, arguments).unwrap_or_else(|e| panic!("{name} failed: {e}"));
        let tool = srv.0.tools.iter().find(|t| t.name == name).expect(name);
        let schema = Value::Object((**tool.output_schema.as_ref().expect("output schema")).clone());
        let validator = jsonschema::validator_for(&schema).expect("valid output schema");
        let errors: Vec<String> = validator
            .iter_errors(&value)
            .map(|e| format!("{}: {e}", e.instance_path))
            .collect();
        assert!(
            errors.is_empty(),
            "{name} result does not match its output schema: {errors:?}\n{value:#}"
        );
        value
    }

    #[test]
    fn every_tool_result_satisfies_its_output_schema() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = repo(dir.path());
        let svc = service(dir.path());
        let alice = serve(&svc, "auth", "alice");
        let bob = serve(&svc, "auth", "bob");
        let ws = json!({ "workspace": root });

        // A plain note: points_at, verdict, reopens and the timestamps are null.
        let plain = checked(
            &alice,
            "add",
            json!({"workspace": root, "file": "app.py", "start_line": 1, "comment": "plain"}),
        );
        assert_eq!(plain["points_at"], Value::Null);
        assert_eq!(plain["verdict"], Value::Null);
        checked(
            &alice,
            "add",
            json!({"workspace": root, "file": "app.py", "start_line": 2, "comment": "pointing",
                   "points_at": {"file": "app.py", "start_line": 3, "end_line": 3}}),
        );
        checked(
            &alice,
            "add",
            json!({"workspace": root, "file": "app.py", "start_line": 3, "comment": "to edit"}),
        );
        checked(
            &alice,
            "add",
            json!({"workspace": root, "file": "app.py", "start_line": 3, "comment": "to remove"}),
        );
        checked(
            &alice,
            "edit",
            json!({"workspace": root, "id": "r3", "comment": "edited"}),
        );
        checked(&alice, "remove", json!({"workspace": root, "id": "r4"}));

        let listed = checked(&alice, "list", json!({"workspace": root, "status": "all"}));
        assert_eq!(listed["comments"][0]["verdict"], Value::Null);
        checked(&alice, "count", ws.clone());
        checked(
            &alice,
            "show_comment",
            json!({"workspace": root, "id": "r2"}),
        );
        checked(&alice, "gate", ws.clone());
        checked(&alice, "workspaces", json!({}));
        checked(&alice, "workspaces", json!({"all": true}));

        checked(
            &alice,
            "submit",
            json!({"workspace": root, "decision": "comment", "summary": "see notes", "comment_ids": ["r1"]}),
        );
        checked(&alice, "list_reviews", ws.clone());
        checked(
            &alice,
            "show_review",
            json!({"workspace": root, "id": "rv1"}),
        );

        checked(&bob, "pull", json!({"workspace": root, "peek": true}));
        let pulled = checked(&bob, "pull", ws.clone());
        assert_eq!(pulled["comments"][0]["points_at"]["file"], "app.py");
        for id in ["r1", "r2", "r3"] {
            checked(
                &bob,
                "resolve",
                json!({"workspace": root, "id": id, "outcome": "rejected", "note": "no"}),
            );
        }

        // A verdict and a reopen fill the other nullable objects.
        let claude = serve(&svc, "", "claude");
        let judged = checked(
            &claude,
            "judge",
            json!({"workspace": root, "id": "r1", "verdict": "reopened", "note": "again"}),
        );
        assert_eq!(judged["comment"]["verdict"]["kind"], "reopened");
        checked(
            &claude,
            "judge",
            json!({"workspace": root, "id": "r2", "verdict": "accepted"}),
        );
        checked(&claude, "list", json!({"workspace": root, "status": "all"}));
        checked(
            &claude,
            "show_comment",
            json!({"workspace": root, "id": "r1"}),
        );
        // A pinned pull that takes nothing reports the work waiting elsewhere.
        checked(
            &alice,
            "add",
            json!({"workspace": root, "file": "app.py", "start_line": 1, "comment": "waits"}),
        );
        let elsewhere = checked(&serve(&svc, "payments", "bob"), "pull", ws.clone());
        assert!(elsewhere.get("elsewhere").is_some(), "{elsewhere:#}");
        checked(
            &claude,
            "override_gate",
            json!({"workspace": root, "action": "push", "reason": "urgent"}),
        );
    }

    #[test]
    fn report_session_then_a_repeat_adds_commits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = repo(dir.path());
        let srv = server(dir.path(), "", "");
        let first = checked(
            &srv,
            "report_session",
            json!({"workspace": root, "session_id": "s1", "tool": "claude-code", "model": "opus",
                   "summary": "fixed the parser", "commits": ["abc1234"]}),
        );
        assert_eq!(first["session"]["commits"], json!(["abc1234"]));

        let repeat = checked(
            &srv,
            "report_session",
            json!({"workspace": root, "session_id": "s1", "tool": "claude-code", "model": "opus",
                   "commits": ["def5678"]}),
        );
        assert_eq!(repeat["session"]["commits"], json!(["abc1234", "def5678"]));
        assert_eq!(repeat["session"]["summary"], "fixed the parser");
        assert_eq!(
            repeat["session"]["reported_at"],
            first["session"]["reported_at"]
        );
    }

    #[test]
    fn report_session_keeps_the_first_tool_and_model() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = repo(dir.path());
        let srv = server(dir.path(), "", "");
        checked(
            &srv,
            "report_session",
            json!({"workspace": root, "session_id": "s1", "tool": "claude-code", "model": "opus"}),
        );
        let err = invoke(
            &srv.0,
            "report_session",
            args(&json!({"workspace": root, "session_id": "s1", "tool": "codex", "model": "opus"})),
        )
        .unwrap_err();
        assert!(err.contains("was reported by claude-code/opus"), "{err}");
    }

    #[test]
    fn report_session_refuses_a_bad_commit_id_and_stores_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = repo(dir.path());
        let srv = server(dir.path(), "", "");
        let err = invoke(
            &srv.0,
            "report_session",
            args(&json!({"workspace": root, "session_id": "s1", "tool": "claude-code", "model": "opus",
                         "commits": ["not-a-commit"]})),
        )
        .unwrap_err();
        assert!(err.contains("is not a commit id"), "{err}");

        // Nothing was stored, so a report under another tool is not a conflict.
        checked(
            &srv,
            "report_session",
            json!({"workspace": root, "session_id": "s1", "tool": "codex", "model": "gpt", "commits": ["abc1234"]}),
        );
    }

    /// Checks that `value` satisfies the schema of `T`, the schema a tool with
    /// that output serves. Used for operations that are not MCP tools.
    fn satisfies<T: JsonSchema>(value: &Value) {
        let schema = Value::Object(object_schema::<T>());
        let validator = jsonschema::validator_for(&schema).expect("valid output schema");
        let errors: Vec<String> = validator
            .iter_errors(value)
            .map(|e| format!("{}: {e}", e.instance_path))
            .collect();
        assert!(
            errors.is_empty(),
            "does not match its output schema: {errors:?}\n{value:#}"
        );
    }

    /// Runs git in `root` and returns its trimmed stdout.
    fn git_in(root: &str, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout)
            .expect("utf8")
            .trim()
            .to_owned()
    }

    /// Commits `app.py` in `root` and returns the new commit's full id.
    fn commit_in(root: &str, message: &str) -> String {
        git_in(root, &["add", "app.py"]);
        git_in(
            root,
            &[
                "-c",
                "user.name=Rvw Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
        git_in(root, &["rev-parse", "HEAD"])
    }

    /// The queue behind the schema checks, built through the Service.
    struct Scenario {
        svc: Arc<Service>,
        root: String,
        /// the first commit, outside the ledger range
        base: String,
        /// a commit with session s1 and the linked comment r1
        linked: String,
        /// a commit with no reported session and the unlinked comment r2
        unreported: String,
    }

    /// Two sessions (s1 without a summary, s2 with one), one linked comment,
    /// one unlinked comment, a lane override in lane auth and a lane-less one,
    /// and a commit with no session.
    fn scenario(dir: &std::path::Path) -> Scenario {
        use rvw_core::operations::ReportSessionInput;

        let root = repo(dir);
        let edit = |text: &str| {
            fs::write(std::path::Path::new(&root).join("app.py"), text).expect("write");
        };
        let base = commit_in(&root, "base");
        let svc = service(dir);
        let alice = serve(&svc, "auth", "alice");
        let bob = serve(&svc, "auth", "bob");

        // r1 is linked: its resolution is the version the next commit records.
        edit("one\nTWO\nthree\n");
        invoke(
            &alice.0,
            "add",
            args(
                &json!({"workspace": root, "file": "app.py", "start_line": 2, "comment": "rename"}),
            ),
        )
        .expect("add r1");
        invoke(&bob.0, "pull", args(&json!({"workspace": root}))).expect("pull r1");
        invoke(
            &bob.0,
            "resolve",
            args(&json!({"workspace": root, "id": "r1", "outcome": "done", "note": "renamed"})),
        )
        .expect("resolve r1");
        let linked = commit_in(&root, "rename two");

        svc.report_session(ReportSessionInput {
            workspace: root.clone(),
            session_id: "s1".into(),
            tool: "claude-code".into(),
            model: "opus".into(),
            commits: vec![linked.clone()],
            ..Default::default()
        })
        .expect("report s1");
        svc.report_session(ReportSessionInput {
            workspace: root.clone(),
            session_id: "s2".into(),
            tool: "codex".into(),
            model: "gpt".into(),
            summary: "set up the file".into(),
            commits: vec![base.clone()],
        })
        .expect("report s2");

        // A lane override while r1 is unjudged in lane auth, then a lane-less one.
        invoke(
            &alice.0,
            "override_gate",
            args(&json!({"workspace": root, "action": "commit", "reason": "shipping the rename"})),
        )
        .expect("lane override");
        let gustav = serve(&svc, "", "gustav");
        invoke(
            &gustav.0,
            "override_gate",
            args(&json!({"workspace": root, "action": "push", "reason": "urgent fix"})),
        )
        .expect("lane-less override");

        // r2 is unlinked: it resolves to a version no commit of the range records.
        edit("one\nTWO\nTHREE\n");
        invoke(
            &alice.0,
            "add",
            args(&json!({"workspace": root, "file": "app.py", "start_line": 3, "comment": "also three"})),
        )
        .expect("add r2");
        invoke(&bob.0, "pull", args(&json!({"workspace": root}))).expect("pull r2");
        invoke(
            &bob.0,
            "resolve",
            args(&json!({"workspace": root, "id": "r2", "outcome": "done", "note": "kept"})),
        )
        .expect("resolve r2");
        edit("one\nTWO\nTHREE!\n");
        let unreported = commit_in(&root, "shout three");

        Scenario {
            svc,
            root,
            base,
            linked,
            unreported,
        }
    }

    #[test]
    fn sessions_values_satisfy_their_output_schema() {
        use rvw_core::operations::{SessionsInput, SessionsOutput};

        let dir = tempfile::tempdir().expect("tempdir");
        let s = scenario(dir.path());
        let sessions = s
            .svc
            .sessions(SessionsInput { workspace: s.root })
            .expect("sessions");
        let value = serde_json::to_value(&sessions).expect("serialize sessions");

        let by_id = |id: &str| {
            value["sessions"]
                .as_array()
                .expect("sessions")
                .iter()
                .find(|x| x["session_id"] == id)
                .unwrap_or_else(|| panic!("no session {id}"))
                .clone()
        };
        assert_eq!(by_id("s1")["summary"], Value::Null);
        assert_eq!(by_id("s2")["summary"], "set up the file");
        satisfies::<SessionsOutput>(&value);
    }

    #[test]
    fn ledger_values_satisfy_their_output_schema() {
        use rvw_core::operations::{LedgerInput, LedgerOutput};

        let dir = tempfile::tempdir().expect("tempdir");
        let s = scenario(dir.path());
        let ledger = s
            .svc
            .ledger(LedgerInput {
                workspace: s.root.clone(),
                range: format!("{}..HEAD", s.base),
                lane: "auth".into(),
                ..Default::default()
            })
            .expect("ledger");
        assert_eq!(ledger.commits.len(), 2);
        assert_eq!(ledger.commits[0].id, s.linked);
        assert_eq!(ledger.commits[0].session_id.as_deref(), Some("s1"));
        assert_eq!(ledger.commits[0].comments.len(), 1);
        assert_eq!(ledger.commits[1].id, s.unreported);
        assert_eq!(ledger.commits[1].session_id, None);
        assert_eq!(ledger.unlinked.len(), 1, "{:?}", ledger.unlinked);
        assert_eq!(ledger.overrides.len(), 1);
        assert_eq!(ledger.overrides[0].lane.as_deref(), Some("auth"));
        assert_eq!(ledger.unscoped_overrides.len(), 1);
        assert_eq!(ledger.unscoped_overrides[0].lane, None);
        assert_eq!(ledger.sessions.len(), 1);

        let value = serde_json::to_value(&ledger).expect("serialize ledger");
        assert_eq!(value["commits"][1]["session_id"], Value::Null);
        assert_eq!(value["unscoped_overrides"][0]["lane"], Value::Null);
        satisfies::<LedgerOutput>(&value);
    }
}
