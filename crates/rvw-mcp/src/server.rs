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
use rvw_core::operations::{
    AddInput, EditInput, GetInput, PullInput, QueryInput, RemoveInput, ResolveInput, SheetsInput,
    SubmitInput, WorkspacesInput,
};
use rvw_core::Service;
use schemars::generate::SchemaSettings;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{Map, Value};

use crate::scope::scope;
use crate::Options;

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

To review code for another agent: add comments on line ranges, then submit
them as one review with a decision (comment, approve, request-changes).

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
        assert!(Command::new("git")
            .arg("-C")
            .arg(&root)
            .args(["init", "-q"])
            .status()
            .expect("git init")
            .success());
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

    #[test]
    fn registers_the_twelve_tools_in_name_order() {
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
                "list",
                "list_reviews",
                "pull",
                "remove",
                "resolve",
                "show_comment",
                "show_review",
                "submit",
                "workspaces"
            ]
        );
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
}
