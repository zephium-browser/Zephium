//! `describe_project`: a granted folder read exactly into a `project`
//! object. The stack comes from manifests, the structure from a bounded
//! walk and the scripts from the files that define them. Git state remains
//! unavailable until a confined runner is qualified; no raw process is launched.
//! A second reading of the same folder revises the first.
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use zephium_core::work::{
    artifact::{WorkArtifactDataV1, WorkEvidenceLink},
    model::{WorkModelTool, WorkModelToolCall},
    objects::*,
    parts::WorkHelperV1,
    runtime::*,
    WorkArtifactId,
};

use super::tools::{
    LeadRunView, LeadScope, LeadToolContext, LeadToolFuture, LeadToolOutcome, LeadToolSet,
};

const MANIFEST_BYTES: u64 = 256 * 1024;
const ROOT_LISTED: usize = 24;
const CHILDREN_LISTED: usize = 8;
const GRANDCHILDREN_LISTED: usize = 6;
const DIR_READ: usize = 2_000;
/// What project templates write as a description: it says nothing.
const TEMPLATE_DESCRIPTIONS: [&str; 4] = [
    "A Tauri App",
    "A Tauri app",
    "Add your description here",
    "A new Flutter project.",
];
/// Folders never shown: build output, caches, dependencies.
const SKIPPED: [&str; 16] = [
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "coverage",
    "__pycache__",
    "venv",
    "vendor",
    "Pods",
    "DerivedData",
    "bower_components",
    "site-packages",
    "tmp",
    "logs",
    "gen",
];
/// Hidden entries still worth showing.
const HIDDEN_SHOWN: [&str; 2] = [".github", ".gitlab-ci.yml"];
/// Folders whose own folders are opened one level further.
const DEEP: [&str; 9] = [
    "src",
    "app",
    "lib",
    "crates",
    "packages",
    "apps",
    "src-tauri",
    "cmd",
    "internal",
];

pub(crate) struct ProjectTools;

impl LeadToolSet for ProjectTools {
    fn tools(&self, scope: LeadScope, run: &LeadRunView<'_>) -> Vec<WorkModelTool> {
        let offered = matches!(
            scope,
            LeadScope::Lead | LeadScope::Helper(WorkHelperV1::Computer)
        );
        if !offered || run.folders().is_empty() {
            return Vec::new();
        }
        vec![WorkModelTool {
            name: "describe_project".into(),
            description: "Reads a granted folder exactly and places it as a project object: what it is, its stack from the manifests, its structure and its scripts. Git state is currently unavailable while a confined runner is being qualified. Use it for what a folder or project is or holds; it is the result for such a request. Reading the same folder again updates the object in place. Returns a short digest with the source key.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string", "description": "The folder's absolute path; the request's folder when omitted."}
                },
                "additionalProperties": false
            }),
            strict: false,
        }]
    }

    fn call<'a>(
        &'a self,
        context: LeadToolContext<'a>,
        call: WorkModelToolCall,
    ) -> LeadToolFuture<'a> {
        Box::pin(async move {
            match describe(context, &call.arguments).await {
                Ok(digest) => LeadToolOutcome::ok(digest),
                Err(fault) => LeadToolOutcome::error(fault),
            }
        })
    }
}

async fn describe(context: LeadToolContext<'_>, args: &Value) -> Result<String, String> {
    let folders = context.run.folders();
    let asked = args
        .get("path")
        .and_then(Value::as_str)
        .map(|p| p.trim().trim_end_matches('/').to_owned())
        .filter(|p| !p.is_empty())
        .or_else(|| folders.current.first().cloned())
        .or_else(|| folders.available.first().cloned())
        .ok_or("No folder is readable in this run")?;
    let files = context.files().ok_or("No folder is readable in this run")?;
    let root = files
        .resolve(&asked, false)
        .map_err(|error| format!("{asked}: {}", error.note()))?;
    if !crate::work_files::directory_available(&root) {
        return Err(format!("{asked} is not a folder"));
    }
    let root_text = root.to_string_lossy().into_owned();
    let step = context
        .begin_step(
            WorkStepKindV1::List {
                path: root_text.clone(),
                depth: Some(2),
            },
            WorkStepStatus::Running,
            None,
            None,
        )
        .await
        .map_err(|_| "The reading could not be recorded".to_owned())?;
    let listing = match files.list_at(&root_text, 2) {
        Ok(listing) => listing,
        Err(error) => {
            let _ = context
                .settle_step(
                    step,
                    WorkStepStatus::Failed,
                    Some(error.note().into()),
                    None,
                )
                .await;
            return Err(format!("{asked}: {}", error.note()));
        }
    };
    let record = WorkFileRecordV1 {
        id: WorkArtifactId::generate(),
        node: context.node(),
        attempt: context.attempt(),
        file: listing,
    };
    let link = WorkEvidenceLink {
        extraction_id: record.id,
        source_id: 1,
    };
    context
        .settle_step(
            step,
            WorkStepStatus::Succeeded,
            Some("Read the project".into()),
            Some(record),
        )
        .await
        .map_err(|_| "The reading could not be recorded".to_owned())?;
    let name = super::folders::name(&root_text);
    let key = context.cite(link.clone(), &name, None);
    let scanned = {
        let root = root.clone();
        tokio::task::spawn_blocking(move || scan(&root))
            .await
            .map_err(|_| "The folder could not be read".to_owned())?
    };
    if scanned.tree.is_empty() {
        return Err(format!("{asked} has no readable project entries"));
    }
    let git = git_state(&root).await;
    let data = scanned.into_data(&root_text, git);
    if let Some(fault) = data.lead_fault(1) {
        return Err(format!(
            "The project could not be described: {}",
            fault.describe()
        ));
    }
    let digest = digest(&data, &key);
    let projection = context
        .probe()
        .runtime_projection()
        .await
        .map_err(|_| "The canvas could not be read".to_owned())?;
    let canvas = super::objects::canvas(&projection, context.execution());
    let earlier = canvas.iter().find(|object| {
        object.current
            && matches!(&object.artifact.data, WorkArtifactDataV1::Project { root, .. } if *root == root_text)
    });
    let title = match &data {
        WorkArtifactDataV1::Project { name, .. } => {
            super::call::clip(name, super::objects::MAX_TITLE_CHARS)
        }
        _ => name,
    };
    let id = super::objects::publish(
        context.run,
        super::objects::Proposed {
            title,
            data,
            evidence: vec![link],
            left_out: vec![],
        },
        context.part(),
        earlier.map(|object| object.artifact.id),
    )
    .await
    .map_err(|_| "The project could not be placed".to_owned())?;
    context.activity(zephium_ipc::work::WorkActivityV1::ProducingArtifact);
    Ok(format!(
        "{} project {id}. {digest}",
        if earlier.is_some() {
            "Updated"
        } else {
            "Placed"
        }
    ))
}

/// The compact text the model reads: what the object shows, and the first
/// lines of the README for the reply.
fn digest(data: &WorkArtifactDataV1, key: &str) -> String {
    let WorkArtifactDataV1::Project {
        name,
        summary,
        stack,
        tree,
        scripts,
        git,
        ..
    } = data
    else {
        return String::new();
    };
    let mut out = format!("[{key}] {name}: {summary}\n");
    if !stack.is_empty() {
        let items: Vec<String> = stack
            .iter()
            .map(|item| match (&item.version, &item.role) {
                (Some(v), Some(r)) => format!("{} {v} ({r})", item.name),
                (None, Some(r)) => format!("{} ({r})", item.name),
                (Some(v), None) => format!("{} {v}", item.name),
                (None, None) => item.name.clone(),
            })
            .collect();
        out.push_str(&format!("Stack: {}\n", items.join(", ")));
    }
    let top: Vec<String> = tree
        .iter()
        .filter(|e| !e.path.contains('/'))
        .map(|e| match e.kind {
            WorkProjectEntryKindV1::Folder => format!("{}/", e.path),
            WorkProjectEntryKindV1::File => e.path.clone(),
        })
        .collect();
    out.push_str(&format!("Top level: {}\n", top.join(", ")));
    if !scripts.is_empty() {
        let listed: Vec<String> = scripts
            .iter()
            .map(|s| format!("{} ({})", s.name, s.command))
            .collect();
        out.push_str(&format!("Scripts: {}\n", listed.join("; ")));
    }
    match git {
        Some(git) => out.push_str(&format!(
            "Git: {}, {} changed{}\n",
            git.branch.as_deref().unwrap_or("detached"),
            git.changed,
            match (git.ahead, git.behind) {
                (Some(a), Some(b)) => format!(", {a} ahead, {b} behind"),
                (Some(a), None) => format!(", {a} ahead"),
                (None, Some(b)) => format!(", {b} behind"),
                (None, None) => String::new(),
            }
        )),
        None => out.push_str("Git state unavailable\n"),
    }
    out.push_str("The object shows all of this; the reply says what the project is and its state in one to three sentences, without restating the object.");
    out
}

#[derive(Default)]
struct Scanned {
    name: Option<String>,
    description: Option<String>,
    readme: Option<String>,
    stack: Vec<WorkProjectStackV1>,
    tree: Vec<WorkProjectEntryV1>,
    more: Option<u32>,
    scripts: Vec<WorkProjectScriptV1>,
    files: usize,
}

impl Scanned {
    fn into_data(self, root: &str, git: Option<WorkProjectGitV1>) -> WorkArtifactDataV1 {
        let folder = super::folders::name(root);
        let name = one_line(self.name.as_deref().unwrap_or(&folder), 60);
        let summary = self
            .description
            .as_deref()
            .filter(|d| {
                !TEMPLATE_DESCRIPTIONS
                    .iter()
                    .any(|t| d.trim().eq_ignore_ascii_case(t))
            })
            .and_then(|d| sentence(d, 160))
            .or_else(|| self.readme.as_deref().and_then(|r| sentence(r, 160)))
            .unwrap_or_else(|| described(&self.stack, self.files));
        let tree = self.tree;
        WorkArtifactDataV1::Project {
            name,
            summary,
            root: root.to_owned(),
            stack: self.stack,
            tree,
            more: self.more,
            scripts: self.scripts,
            git,
        }
    }
}

/// "A Tauri desktop app with SvelteKit", from the stack when nothing says
/// what the project is.
fn described(stack: &[WorkProjectStackV1], files: usize) -> String {
    let role = |role: &str| {
        stack
            .iter()
            .find(|item| item.role.as_deref() == Some(role))
            .map(|item| item.name.clone())
    };
    match (
        role("Desktop shell"),
        role("Framework").or_else(|| role("UI")),
        role("Language"),
    ) {
        (Some(shell), Some(ui), _) => format!("A {shell} desktop app with {ui}"),
        (Some(shell), None, _) => format!("A {shell} desktop app"),
        (None, Some(ui), _) => format!("A {ui} app"),
        (None, None, Some(language)) => format!("A {language} project"),
        _ => format!("A folder of {files} files"),
    }
}

fn one_line(text: &str, max: usize) -> String {
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let mut out = String::new();
    for word in line.split(' ') {
        if out.chars().count() + word.chars().count() + 1 > max {
            break;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// The first sentence of prose that fits, never cut mid-word; Markdown
/// headings, badges and links are passed over.
fn sentence(text: &str, max: usize) -> Option<String> {
    let prose = text
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty()
                && !line.starts_with('#')
                && !line.starts_with('[')
                && !line.starts_with('!')
                && !line.starts_with('<')
                && !line.starts_with('`')
                && !line.starts_with('|')
                && !line.starts_with("---")
        })
        .take(3)
        .collect::<Vec<_>>()
        .join(" ");
    let prose = prose.replace("**", "").replace('`', "");
    if prose.is_empty() {
        return None;
    }
    let first = match prose.find(". ") {
        Some(end) => &prose[..=end],
        None => prose.as_str(),
    };
    let line = one_line(first.trim(), max);
    (line.chars().count() >= 8).then_some(line)
}

fn read(path: &Path) -> Option<String> {
    crate::work_files::read_text_limited(path, MANIFEST_BYTES)
        .ok()?
        .2
}

/// Reads the folder: manifests at the root and one level down, the tree and
/// the scripts. Symlinks are never followed.
fn scan(root: &Path) -> Scanned {
    let mut scanned = Scanned::default();
    let mut stack = Stack::default();
    let manifests: Vec<PathBuf> = std::iter::once(root.to_path_buf())
        .chain(
            [
                "src-tauri",
                "frontend",
                "backend",
                "web",
                "app",
                "server",
                "client",
            ]
            .iter()
            .map(|dir| root.join(dir))
            .filter(|dir| crate::work_files::directory_available(dir)),
        )
        .collect();
    for dir in &manifests {
        let relative = |file: &str| {
            let path = dir.join(file);
            path.strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .into_owned()
        };
        if let Some(text) = read(&dir.join("package.json")) {
            package_json(&text, &relative("package.json"), &mut stack, &mut scanned);
            for (lock, name, host) in [
                ("pnpm-lock.yaml", "pnpm", "pnpm.io"),
                ("yarn.lock", "Yarn", "yarnpkg.com"),
                ("bun.lockb", "Bun", "bun.sh"),
                ("bun.lock", "Bun", "bun.sh"),
                ("package-lock.json", "npm", "npmjs.com"),
            ] {
                if crate::work_files::file_available(&dir.join(lock)) {
                    stack.add(name, None, "Package manager", Some(host), &relative(lock));
                    break;
                }
            }
        }
        if let Some(text) = read(&dir.join("Cargo.toml")) {
            cargo_toml(&text, &relative("Cargo.toml"), &mut stack, &mut scanned);
        }
        if let Some(text) = read(&dir.join("pyproject.toml")) {
            pyproject(&text, &relative("pyproject.toml"), &mut stack, &mut scanned);
        } else if let Some(text) = read(&dir.join("requirements.txt")) {
            stack.add(
                "Python",
                None,
                "Language",
                Some("python.org"),
                &relative("requirements.txt"),
            );
            for line in text.lines() {
                let name = requirement(line);
                if !name.is_empty() {
                    stack.known(&name, None, &relative("requirements.txt"), PYTHON);
                }
            }
        }
        if let Some(text) = read(&dir.join("go.mod")) {
            go_mod(&text, &relative("go.mod"), &mut stack, &mut scanned);
        }
        if let Some(text) = read(&dir.join("tauri.conf.json")) {
            let json: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            stack.add(
                "Tauri",
                None,
                "Desktop shell",
                Some("tauri.app"),
                &relative("tauri.conf.json"),
            );
            if let Some(product) = json
                .get("productName")
                .or_else(|| json.pointer("/package/productName"))
                .and_then(Value::as_str)
            {
                scanned.name = Some(product.to_owned());
            }
        }
        if crate::work_files::file_available(&dir.join("Gemfile")) {
            stack.add(
                "Ruby",
                None,
                "Language",
                Some("ruby-lang.org"),
                &relative("Gemfile"),
            );
            if read(&dir.join("Gemfile"))
                .is_some_and(|t| t.contains("'rails'") || t.contains("\"rails\""))
            {
                stack.add(
                    "Rails",
                    None,
                    "Framework",
                    Some("rubyonrails.org"),
                    &relative("Gemfile"),
                );
            }
        }
        if crate::work_files::file_available(&dir.join("deno.json"))
            || crate::work_files::file_available(&dir.join("deno.jsonc"))
        {
            stack.add(
                "Deno",
                None,
                "Runtime",
                Some("deno.com"),
                &relative("deno.json"),
            );
        }
    }
    if crate::work_files::file_available(&root.join("Dockerfile")) {
        stack.add(
            "Docker",
            None,
            "Container",
            Some("docker.com"),
            "Dockerfile",
        );
    }
    if crate::work_files::directory_available(&root.join(".github/workflows")) {
        stack.add(
            "GitHub Actions",
            None,
            "CI",
            Some("github.com"),
            ".github/workflows",
        );
    }
    scanned.stack = stack.finish();
    scanned.readme = [
        "README.md",
        "README",
        "readme.md",
        "Readme.md",
        "README.rst",
        "README.txt",
    ]
    .iter()
    .find_map(|name| read(&root.join(name)));
    scripts(root, &mut scanned);
    tree(root, &mut scanned);
    scanned
}

/// The stack as it is found, one entry per name, ordered by role.
#[derive(Default)]
struct Stack(Vec<(usize, WorkProjectStackV1)>);

type Known = &'static [(
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
)];

/// Dependency → (name, role, host).
const JS: Known = &[
    (
        "@sveltejs/kit",
        "SvelteKit",
        "Framework",
        Some("svelte.dev"),
    ),
    ("svelte", "Svelte", "UI", Some("svelte.dev")),
    ("next", "Next.js", "Framework", Some("nextjs.org")),
    ("react", "React", "UI", Some("react.dev")),
    ("nuxt", "Nuxt", "Framework", Some("nuxt.com")),
    ("vue", "Vue", "UI", Some("vuejs.org")),
    ("@angular/core", "Angular", "Framework", Some("angular.dev")),
    ("solid-js", "Solid", "UI", Some("solidjs.com")),
    ("astro", "Astro", "Framework", Some("astro.build")),
    ("@remix-run/react", "Remix", "Framework", Some("remix.run")),
    ("expo", "Expo", "Framework", Some("expo.dev")),
    (
        "react-native",
        "React Native",
        "UI",
        Some("reactnative.dev"),
    ),
    (
        "electron",
        "Electron",
        "Desktop shell",
        Some("electronjs.org"),
    ),
    (
        "@tauri-apps/api",
        "Tauri",
        "Desktop shell",
        Some("tauri.app"),
    ),
    (
        "@tauri-apps/cli",
        "Tauri",
        "Desktop shell",
        Some("tauri.app"),
    ),
    ("express", "Express", "Server", Some("expressjs.com")),
    ("fastify", "Fastify", "Server", Some("fastify.dev")),
    ("hono", "Hono", "Server", Some("hono.dev")),
    ("@nestjs/core", "NestJS", "Server", Some("nestjs.com")),
    ("@prisma/client", "Prisma", "Database", Some("prisma.io")),
    ("prisma", "Prisma", "Database", Some("prisma.io")),
    (
        "drizzle-orm",
        "Drizzle",
        "Database",
        Some("orm.drizzle.team"),
    ),
    (
        "@supabase/supabase-js",
        "Supabase",
        "Database",
        Some("supabase.com"),
    ),
    (
        "firebase",
        "Firebase",
        "Database",
        Some("firebase.google.com"),
    ),
    ("mongoose", "MongoDB", "Database", Some("mongodb.com")),
    ("openai", "OpenAI", "AI", Some("openai.com")),
    (
        "@anthropic-ai/sdk",
        "Anthropic",
        "AI",
        Some("anthropic.com"),
    ),
    ("ai", "AI SDK", "AI", Some("vercel.com")),
    ("stripe", "Stripe", "Payments", Some("stripe.com")),
    (
        "tailwindcss",
        "Tailwind CSS",
        "Styling",
        Some("tailwindcss.com"),
    ),
    ("three", "three.js", "Graphics", Some("threejs.org")),
    (
        "typescript",
        "TypeScript",
        "Language",
        Some("typescriptlang.org"),
    ),
    ("vite", "Vite", "Build", Some("vite.dev")),
    ("webpack", "webpack", "Build", Some("webpack.js.org")),
    ("vitest", "Vitest", "Tests", Some("vitest.dev")),
    ("jest", "Jest", "Tests", Some("jestjs.io")),
    (
        "@playwright/test",
        "Playwright",
        "Tests",
        Some("playwright.dev"),
    ),
];
const RUST: Known = &[
    ("tauri", "Tauri", "Desktop shell", Some("tauri.app")),
    ("tokio", "Tokio", "Runtime", Some("tokio.rs")),
    ("axum", "Axum", "Server", None),
    ("actix-web", "Actix Web", "Server", Some("actix.rs")),
    ("rocket", "Rocket", "Server", Some("rocket.rs")),
    ("leptos", "Leptos", "UI", Some("leptos.dev")),
    ("bevy", "Bevy", "Engine", Some("bevyengine.org")),
    ("sqlx", "SQLx", "Database", None),
    ("diesel", "Diesel", "Database", Some("diesel.rs")),
    ("rusqlite", "SQLite", "Database", Some("sqlite.org")),
    ("wgpu", "wgpu", "Graphics", Some("wgpu.rs")),
];
const PYTHON: Known = &[
    ("django", "Django", "Framework", Some("djangoproject.com")),
    (
        "flask",
        "Flask",
        "Framework",
        Some("flask.palletsprojects.com"),
    ),
    (
        "fastapi",
        "FastAPI",
        "Framework",
        Some("fastapi.tiangolo.com"),
    ),
    ("torch", "PyTorch", "AI", Some("pytorch.org")),
    ("tensorflow", "TensorFlow", "AI", Some("tensorflow.org")),
    ("transformers", "Transformers", "AI", Some("huggingface.co")),
    ("langchain", "LangChain", "AI", Some("langchain.com")),
    ("openai", "OpenAI", "AI", Some("openai.com")),
    ("anthropic", "Anthropic", "AI", Some("anthropic.com")),
    ("numpy", "NumPy", "Library", Some("numpy.org")),
    ("pandas", "pandas", "Library", Some("pandas.pydata.org")),
    ("pydantic", "Pydantic", "Library", Some("pydantic.dev")),
    (
        "sqlalchemy",
        "SQLAlchemy",
        "Database",
        Some("sqlalchemy.org"),
    ),
    ("pytest", "pytest", "Tests", Some("pytest.org")),
];
const GO: Known = &[
    (
        "github.com/gin-gonic/gin",
        "Gin",
        "Server",
        Some("gin-gonic.com"),
    ),
    (
        "github.com/labstack/echo",
        "Echo",
        "Server",
        Some("echo.labstack.com"),
    ),
    (
        "github.com/gofiber/fiber",
        "Fiber",
        "Server",
        Some("gofiber.io"),
    ),
];
const ROLES: [&str; 18] = [
    "Language",
    "Framework",
    "UI",
    "Desktop shell",
    "Server",
    "Runtime",
    "Engine",
    "Database",
    "AI",
    "Payments",
    "Graphics",
    "Styling",
    "Library",
    "Build",
    "Tests",
    "Package manager",
    "Container",
    "CI",
];

impl Stack {
    fn add(
        &mut self,
        name: &str,
        version: Option<&str>,
        role: &str,
        host: Option<&str>,
        manifest: &str,
    ) {
        if self
            .0
            .iter()
            .any(|(_, item)| item.name.eq_ignore_ascii_case(name))
        {
            return;
        }
        let order = ROLES.iter().position(|r| *r == role).unwrap_or(ROLES.len());
        let version = version
            .map(|v| {
                v.trim_start_matches(['^', '~', '=', '>', '<', ' ', 'v'])
                    .to_owned()
            })
            .map(|v| v.split([',', ' ']).next().unwrap_or("").to_owned())
            .filter(|v| {
                !v.is_empty()
                    && v.len() <= 24
                    && v.chars().next().is_some_and(|c| c.is_ascii_digit())
            });
        self.0.push((
            order,
            WorkProjectStackV1 {
                name: name.to_owned(),
                version,
                role: Some(role.to_owned()),
                host: host.map(str::to_owned),
                manifest: Some(manifest.to_owned())
                    .filter(|m| m.len() <= 160 && !m.starts_with('/')),
            },
        ));
    }
    fn known(&mut self, dependency: &str, version: Option<&str>, manifest: &str, known: Known) {
        if let Some((_, name, role, host)) = known.iter().find(|(key, ..)| {
            dependency.eq_ignore_ascii_case(key) || dependency.starts_with(&format!("{key}/"))
        }) {
            self.add(name, version, role, *host, manifest);
        }
    }
    fn finish(mut self) -> Vec<WorkProjectStackV1> {
        self.0.sort_by_key(|(order, _)| *order);
        self.0
            .into_iter()
            .map(|(_, item)| item)
            .take(MAX_PROJECT_STACK)
            .collect()
    }
}

fn package_json(text: &str, manifest: &str, stack: &mut Stack, scanned: &mut Scanned) {
    let Ok(json) = serde_json::from_str::<Value>(text) else {
        return;
    };
    if scanned.description.is_none() {
        scanned.description = json
            .get("description")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .map(str::to_owned);
    }
    let mut typescript = false;
    for field in ["dependencies", "devDependencies"] {
        if let Some(Value::Object(deps)) = json.get(field) {
            for (name, version) in deps {
                typescript |= name == "typescript";
                stack.known(name, version.as_str(), manifest, JS);
            }
        }
    }
    if !typescript {
        stack.add("JavaScript", None, "Language", None, manifest);
    }
    if let Some(Value::Object(scripts)) = json.get("scripts") {
        let runner = "package.json";
        for (name, command) in scripts.iter().take(MAX_PROJECT_SCRIPTS) {
            if let Some(command) = command.as_str() {
                push_script(scanned, name, command, runner);
            }
        }
    }
}

/// One `key = value` of a TOML file, with the table it sits in.
struct TomlEntry {
    table: String,
    key: String,
    value: String,
}

/// Enough TOML for manifests: tables, dotted headers, keys, strings, inline
/// tables and arrays that span lines.
fn toml_entries(text: &str) -> Vec<TomlEntry> {
    let mut out = Vec::new();
    let mut table = String::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            table = line
                .trim_start_matches('[')
                .split(']')
                .next()
                .unwrap_or("")
                .trim()
                .to_owned();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let mut value = value.trim().to_owned();
        let depth = |v: &str| {
            let mut depth = 0i32;
            let mut quoted: Option<char> = None;
            for c in v.chars() {
                match (quoted, c) {
                    (Some(q), c) if c == q => quoted = None,
                    (Some(_), _) => {}
                    (None, '"' | '\'') => quoted = Some(c),
                    (None, '[' | '{') => depth += 1,
                    (None, ']' | '}') => depth -= 1,
                    (None, '#') => break,
                    _ => {}
                }
            }
            depth
        };
        let mut open = depth(&value);
        while open > 0 {
            let Some(next) = lines.next() else { break };
            let next = next.trim();
            if next.starts_with('#') {
                continue;
            }
            value.push(' ');
            value.push_str(next);
            open = depth(&value);
        }
        out.push(TomlEntry {
            table: table.clone(),
            key: key.trim().trim_matches('"').to_owned(),
            value,
        });
    }
    out
}

/// The first quoted string in a TOML value.
fn toml_string(value: &str) -> Option<String> {
    let start = value.find(['"', '\''])?;
    let quote = value[start..].chars().next()?;
    let rest = &value[start + 1..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_owned())
}
/// A dependency's version: `"1.2"` or `{ version = "1.2", … }`.
fn toml_version(value: &str) -> Option<String> {
    if value.starts_with('{') {
        let at = value.find("version")?;
        return toml_string(&value[at..]);
    }
    toml_string(value)
}
/// Every quoted string in a TOML array.
fn toml_strings(value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(found) = toml_string(rest) {
        let at = rest
            .find(&found)
            .map(|i| i + found.len() + 1)
            .unwrap_or(rest.len());
        out.push(found);
        rest = &rest[at.min(rest.len())..];
    }
    out
}

fn cargo_toml(text: &str, manifest: &str, stack: &mut Stack, scanned: &mut Scanned) {
    let entries = toml_entries(text);
    let get = |table: &str, key: &str| {
        entries
            .iter()
            .find(|e| e.table == table && e.key == key)
            .and_then(|e| toml_string(&e.value))
    };
    let rust = get("package", "rust-version");
    stack.add(
        "Rust",
        rust.as_deref(),
        "Language",
        Some("rust-lang.org"),
        manifest,
    );
    if scanned.description.is_none() {
        scanned.description = get("package", "description").filter(|d| !d.trim().is_empty());
    }
    for entry in &entries {
        if matches!(
            entry.table.as_str(),
            "dependencies" | "workspace.dependencies" | "dev-dependencies" | "build-dependencies"
        ) {
            stack.known(
                &entry.key,
                toml_version(&entry.value).as_deref(),
                manifest,
                RUST,
            );
        }
    }
}

fn pyproject(text: &str, manifest: &str, stack: &mut Stack, scanned: &mut Scanned) {
    let entries = toml_entries(text);
    let get = |table: &str, key: &str| {
        entries
            .iter()
            .find(|e| e.table == table && e.key == key)
            .and_then(|e| toml_string(&e.value))
    };
    let python = get("project", "requires-python");
    stack.add(
        "Python",
        python.as_deref(),
        "Language",
        Some("python.org"),
        manifest,
    );
    if scanned.description.is_none() {
        scanned.description =
            get("project", "description").or_else(|| get("tool.poetry", "description"));
    }
    for entry in &entries {
        if entry.table == "project" && entry.key == "dependencies" {
            for requirement_line in toml_strings(&entry.value) {
                stack.known(&requirement(&requirement_line), None, manifest, PYTHON);
            }
        }
        if entry.table == "tool.poetry.dependencies" {
            stack.known(
                &entry.key,
                toml_version(&entry.value).as_deref(),
                manifest,
                PYTHON,
            );
        }
        if entry.table == "project.scripts" {
            push_script(scanned, &entry.key, &entry.key, "pyproject.toml");
        }
    }
}

/// A requirement's package name: "fastapi[all]>=0.110" → "fastapi".
fn requirement(line: &str) -> String {
    let line = line.trim();
    if line.starts_with('#') || line.starts_with('-') {
        return String::new();
    }
    line.split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_' || c == '.'))
        .next()
        .unwrap_or("")
        .to_lowercase()
}

fn go_mod(text: &str, manifest: &str, stack: &mut Stack, _scanned: &mut Scanned) {
    let version = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("go "))
        .map(str::trim);
    stack.add("Go", version, "Language", Some("go.dev"), manifest);
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let first = words.next().unwrap_or("");
        let (module, version) = if first == "require" {
            (words.next().unwrap_or(""), words.next())
        } else {
            (first, words.next())
        };
        stack.known(module, version, manifest, GO);
    }
}

fn push_script(scanned: &mut Scanned, name: &str, command: &str, source: &str) {
    if scanned.scripts.len() >= MAX_PROJECT_SCRIPTS
        || scanned.scripts.iter().any(|s| s.name == name)
    {
        return;
    }
    let name = one_line(name, 32);
    let command = one_line(command, 160);
    if name.is_empty() || command.is_empty() {
        return;
    }
    scanned.scripts.push(WorkProjectScriptV1 {
        name,
        command,
        source: Some(source.to_owned()),
    });
}

/// Make and just targets, and the usual commands of a Cargo or Go project
/// that defines none of its own.
fn scripts(root: &Path, scanned: &mut Scanned) {
    if let Some(text) = read(&root.join("Makefile")) {
        for line in text.lines() {
            if let Some((target, rest)) = line.split_once(':') {
                let plain = !target.is_empty()
                    && !target.starts_with(['.', '\t', ' ', '#'])
                    && target
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                    && !rest.starts_with('=');
                if plain {
                    push_script(scanned, target, &format!("make {target}"), "Makefile");
                }
            }
        }
    }
    if let Some(text) = ["justfile", "Justfile", ".justfile"]
        .iter()
        .find_map(|name| read(&root.join(name)))
    {
        for line in text.lines() {
            if line.starts_with([' ', '\t', '#', '@']) {
                continue;
            }
            if let Some((head, rest)) = line.split_once(':') {
                let name = head.split_whitespace().next().unwrap_or("");
                if !name.is_empty()
                    && !rest.starts_with('=')
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                {
                    push_script(scanned, name, &format!("just {name}"), "justfile");
                }
            }
        }
    }
    if scanned.scripts.is_empty() {
        if crate::work_files::file_available(&root.join("Cargo.toml")) {
            for (name, command) in [
                ("build", "cargo build"),
                ("test", "cargo test"),
                ("run", "cargo run"),
            ] {
                push_script(scanned, name, command, "Cargo");
            }
        } else if crate::work_files::file_available(&root.join("go.mod")) {
            for (name, command) in [("build", "go build ./..."), ("test", "go test ./...")] {
                push_script(scanned, name, command, "Go");
            }
        }
    }
}

struct Entry {
    name: String,
    folder: bool,
}

/// A folder's shown entries: folders first, then files, by name; hidden
/// and generated ones left out. The count is of everything shown.
fn entries(dir: &Path) -> Vec<Entry> {
    let Ok(read) = crate::work_files::read_directory(dir) else {
        return Vec::new();
    };
    let mut out: Vec<Entry> = read
        .take(DIR_READ)
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let kind = entry.file_type().ok()?;
            if kind.is_symlink()
                || (name.starts_with('.') && !HIDDEN_SHOWN.contains(&name.as_str()))
                || (kind.is_dir() && SKIPPED.contains(&name.as_str()))
                || name.chars().any(char::is_control)
                || name.chars().count() > 120
            {
                return None;
            }
            Some(Entry {
                name,
                folder: kind.is_dir(),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.folder
            .cmp(&a.folder)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    out
}

/// Up to three levels, parents first: the root's entries, each folder's
/// first entries, and one level more inside source folders. What is left
/// out is counted on its folder.
fn tree(root: &Path, scanned: &mut Scanned) {
    let top = entries(root);
    scanned.files = top.iter().filter(|e| !e.folder).count();
    let listed = top.len().min(ROOT_LISTED);
    scanned.more = u32::try_from(top.len() - listed).ok().filter(|n| *n > 0);
    let mut tree: Vec<WorkProjectEntryV1> = Vec::new();
    let mut folders: Vec<(String, PathBuf)> = Vec::new();
    for entry in top.into_iter().take(listed) {
        if entry.folder {
            folders.push((entry.name.clone(), root.join(&entry.name)));
        }
        tree.push(WorkProjectEntryV1 {
            path: entry.name,
            kind: kind(entry.folder),
            more: None,
        });
    }
    let mut deeper: Vec<(String, PathBuf)> = Vec::new();
    for (path, dir) in folders {
        let inside = entries(&dir);
        let room = MAX_PROJECT_TREE.saturating_sub(tree.len());
        let show = inside.len().min(CHILDREN_LISTED).min(room);
        let rest = inside.len() - show;
        for entry in inside.into_iter().take(show) {
            let child = format!("{path}/{}", entry.name);
            if entry.folder && DEEP.contains(&path.as_str()) {
                deeper.push((child.clone(), dir.join(&entry.name)));
            }
            tree.push(WorkProjectEntryV1 {
                path: child,
                kind: kind(entry.folder),
                more: None,
            });
        }
        set_more(&mut tree, &path, rest);
    }
    for (path, dir) in deeper {
        let inside = entries(&dir);
        let room = MAX_PROJECT_TREE.saturating_sub(tree.len());
        let show = inside.len().min(GRANDCHILDREN_LISTED).min(room);
        let rest = inside.len() - show;
        for entry in inside.into_iter().take(show) {
            tree.push(WorkProjectEntryV1 {
                path: format!("{path}/{}", entry.name),
                kind: kind(entry.folder),
                more: None,
            });
        }
        set_more(&mut tree, &path, rest);
    }
    // Folders not opened hold what they hold.
    for at in 0..tree.len() {
        if tree[at].kind == WorkProjectEntryKindV1::Folder && tree[at].more.is_none() {
            let prefix = format!("{}/", tree[at].path);
            if !tree.iter().any(|e| e.path.starts_with(&prefix)) {
                let count = entries(&root.join(&tree[at].path)).len();
                tree[at].more = u32::try_from(count).ok().filter(|n| *n > 0);
            }
        }
    }
    scanned.tree = tree;
}

fn kind(folder: bool) -> WorkProjectEntryKindV1 {
    if folder {
        WorkProjectEntryKindV1::Folder
    } else {
        WorkProjectEntryKindV1::File
    }
}

fn set_more(tree: &mut [WorkProjectEntryV1], path: &str, rest: usize) {
    if let Some(entry) = tree.iter_mut().find(|e| e.path == path) {
        entry.more = u32::try_from(rest).ok().filter(|n| *n > 0);
    }
}

/// This field must come from a qualified typed runner. Git status can execute
/// checkout filters and read included configuration; a read-looking argv and
/// the generic command approval gate do not confine this independent caller.
async fn git_state(_root: &Path) -> Option<WorkProjectGitV1> {
    None
}

#[cfg(test)]
fn parse_status(text: &str) -> Option<WorkProjectGitV1> {
    let mut lines = text.lines();
    let head = lines.next()?.strip_prefix("## ")?;
    let head = head.strip_prefix("No commits yet on ").unwrap_or(head);
    let (branch, tracking) = match head.split_once("...") {
        Some((branch, rest)) => (branch, rest),
        None => (head.split(' ').next().unwrap_or(head), head),
    };
    let branch = if branch.starts_with("HEAD (no branch)") || branch == "HEAD" {
        None
    } else {
        Some(branch.to_owned())
    };
    let count = |word: &str| {
        tracking
            .split_once(word)
            .and_then(|(_, rest)| rest.trim().split([',', ']']).next())
            .and_then(|n| n.trim().parse::<u32>().ok())
    };
    Some(WorkProjectGitV1 {
        branch: branch.filter(|b| !b.is_empty() && b.chars().count() <= 80),
        changed: u32::try_from(lines.filter(|l| !l.trim().is_empty()).count()).unwrap_or(u32::MAX),
        ahead: count("ahead "),
        behind: count("behind "),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> (tempfile::TempDir, PathBuf) {
        let scratch = crate::work_files::test_tempdir();
        let root = scratch.path().join("project");
        for dir in [
            "src/routes",
            "src/lib",
            "src-tauri/src",
            "static",
            "node_modules/x",
            ".git",
        ] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"lunios","description":"","scripts":{"dev":"vite dev","build":"vite build"},
            "devDependencies":{"@sveltejs/kit":"^2.8.0","svelte":"^5.1.0","typescript":"~5.6.2","vite":"^6.0.3","@tauri-apps/cli":"^2"}}"#,
        )
        .unwrap();
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        std::fs::write(
            root.join("src-tauri/Cargo.toml"),
            "[package]\nname = \"lunios\"\ndescription = \"A Tauri App\"\n\n[dependencies]\ntauri = { version = \"2\", features = [\n  \"tray-icon\",\n] }\nserde = \"1\"\n",
        )
        .unwrap();
        std::fs::write(
            root.join("src-tauri/tauri.conf.json"),
            r#"{"productName":"Lunios","version":"0.1.0"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("README.md"),
            "# Lunios\n\n![badge](x)\nLunios keeps your moon notes. It syncs.\n",
        )
        .unwrap();
        for file in [
            "src/app.html",
            "src/routes/+page.svelte",
            "src-tauri/src/main.rs",
            "src-tauri/src/lib.rs",
        ] {
            std::fs::write(root.join(file), "").unwrap();
        }
        (scratch, root.canonicalize().unwrap())
    }

    #[test]
    fn a_folder_reads_as_its_stack_structure_and_scripts() {
        let (_scratch, root) = project();
        let scanned = scan(&root);
        let data = scanned.into_data(&root.to_string_lossy(), None);
        assert_eq!(data.lead_fault(1), None);
        let WorkArtifactDataV1::Project {
            name,
            summary,
            stack,
            tree,
            scripts,
            ..
        } = &data
        else {
            panic!("project")
        };
        assert_eq!(name, "Lunios");
        assert_eq!(summary, "Lunios keeps your moon notes.");
        let names: Vec<&str> = stack.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "TypeScript",
                "Rust",
                "SvelteKit",
                "Svelte",
                "Tauri",
                "Vite",
                "pnpm"
            ]
        );
        let kit = stack.iter().find(|s| s.name == "SvelteKit").unwrap();
        assert_eq!(kit.version.as_deref(), Some("2.8.0"));
        assert_eq!(kit.host.as_deref(), Some("svelte.dev"));
        let tauri = stack.iter().find(|s| s.name == "Tauri").unwrap();
        assert_eq!(tauri.manifest.as_deref(), Some("package.json"));
        let paths: Vec<&str> = tree.iter().map(|e| e.path.as_str()).collect();
        assert!(paths.contains(&"src/routes/+page.svelte"));
        assert!(paths.contains(&"src-tauri/src/main.rs"));
        assert!(!paths
            .iter()
            .any(|p| p.contains("node_modules") || p.starts_with(".git")));
        assert_eq!(scripts.len(), 2);
        assert_eq!(scripts[0].source.as_deref(), Some("package.json"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn project_scan_refuses_linked_manifests_and_protected_names() {
        use std::os::unix::fs::symlink;
        let scratch = tempfile::tempdir().unwrap();
        let base = scratch.path().canonicalize().unwrap();
        let root = base.join("project");
        let outside = base.join("outside");
        std::fs::create_dir_all(root.join(".github")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(
            outside.join("package.json"),
            r#"{"name":"outside-fixture-marker","description":"outside credential fixture"}"#,
        )
        .unwrap();
        std::fs::write(
            outside.join("Cargo.toml"),
            "[package]\nname = \"outside-fixture-marker\"\n",
        )
        .unwrap();
        std::fs::write(outside.join("private.txt"), "outside-fixture-marker").unwrap();
        symlink(outside.join("package.json"), root.join("package.json")).unwrap();
        std::fs::hard_link(outside.join("private.txt"), root.join("README.md")).unwrap();
        symlink(&outside, root.join("backend")).unwrap();
        symlink(&outside, root.join(".github/workflows")).unwrap();
        for name in [
            ".env.example",
            ".ENV.PRODUCTION",
            "credentials.json",
            "private.pem",
        ] {
            std::fs::write(root.join(name), "protected-fixture-marker").unwrap();
        }
        std::fs::write(root.join("notes.txt"), "ordinary project fixture").unwrap();
        let data = scan(&root).into_data(&root.to_string_lossy(), None);
        let encoded = serde_json::to_string(&data).unwrap();
        assert!(!encoded.contains("outside-fixture-marker"));
        assert!(!encoded.contains("protected-fixture-marker"));
        assert!(
            !encoded.contains(".env.example")
                && !encoded.contains("credentials.json")
                && !encoded.contains("private.pem")
        );
        assert!(!encoded.contains("GitHub Actions"));
        assert!(encoded.contains("notes.txt"));
        // A parent replaced after grant validation cannot redirect the scan.
        std::fs::rename(&root, base.join("original")).unwrap();
        symlink(&outside, &root).unwrap();
        let scanned = scan(&root);
        assert!(scanned.tree.is_empty() && scanned.readme.is_none() && scanned.stack.is_empty());
    }

    #[tokio::test]
    async fn project_git_is_unavailable_without_claiming_no_repository() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().canonicalize().unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();
        std::fs::write(
            root.join("README.md"),
            "# Project\nOrdinary project fixture.\n",
        )
        .unwrap();
        assert!(git_state(&root).await.is_none());
        let data = scan(&root).into_data(&root.to_string_lossy(), None);
        let text = digest(&data, "s1");
        assert!(text.contains("Git state unavailable"));
        assert!(!text.contains("Not a git repository"));
    }

    #[test]
    fn a_readme_gives_the_summary_when_the_manifests_say_nothing() {
        assert_eq!(
            sentence(
                "# Lunios\n\n![badge](x)\nLunios keeps your moon notes. It syncs.\n",
                160
            )
            .as_deref(),
            Some("Lunios keeps your moon notes.")
        );
        assert_eq!(sentence("# Title only\n", 160), None);
    }

    #[test]
    fn git_status_gives_branch_changes_and_distance() {
        let git =
            parse_status("## main...origin/main [ahead 2, behind 1]\n M src/lib.rs\n?? notes.md\n")
                .unwrap();
        assert_eq!(git.branch.as_deref(), Some("main"));
        assert_eq!(git.changed, 2);
        assert_eq!((git.ahead, git.behind), (Some(2), Some(1)));
        let fresh = parse_status("## No commits yet on main\n").unwrap();
        assert_eq!(fresh.branch.as_deref(), Some("main"));
        assert_eq!(fresh.changed, 0);
        let detached = parse_status("## HEAD (no branch)\n").unwrap();
        assert_eq!(detached.branch, None);
    }

    #[test]
    fn manifests_read_as_toml_without_a_parser_dependency() {
        let entries = toml_entries("[package]\nname = \"a\" # comment\n[dependencies]\ntokio = { version = \"1.40\", features = [\"full\"] }\n[project]\ndependencies = [\n  \"fastapi>=0.110\",\n  \"pydantic\",\n]\n");
        assert_eq!(toml_string(&entries[0].value).as_deref(), Some("a"));
        assert_eq!(toml_version(&entries[1].value).as_deref(), Some("1.40"));
        assert_eq!(
            toml_strings(&entries[2].value),
            ["fastapi>=0.110", "pydantic"]
        );
        assert_eq!(requirement("fastapi[all]>=0.110"), "fastapi");
    }
}
