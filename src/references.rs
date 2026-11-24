use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};

use regex::Regex;
use serde_json::{Value, json};

use crate::comments::Lines;

static RUST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_]\w*)\s*;").unwrap()
});
static JS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
    r#"(?m)^[ \t]*(?:import\s*(?:[\w$*,{}\s]+\s+from\s*)?|export\s+[\w$*,{}\s]+\s+from\s*)["']([^"'\r\n]+)["']"#
).unwrap()
});
static CRATE_USE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(?:pub\s+)?use\s+(crate::[A-Za-z_]\w*)").unwrap());
static PY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^[ \t]*from[ \t]+(\.+[\w.]*)[ \t]+import[ \t]+([^#;\r\n]+)").unwrap()
});

#[derive(Default)]
pub struct Imports {
    paths: Vec<String>,
    declarations: Vec<String>,
    symbols: Vec<String>,
    namespaces: Vec<String>,
    words: HashSet<String>,
    packages: Vec<String>,
}

macro_rules! pattern {
    ($name:ident, $pattern:literal) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).unwrap());
    };
}

pattern!(HEADER, r#"(?m)^[ \t]*#[ \t]*include[ \t]*"([^"\r\n]+)""#);
pattern!(
    BASH,
    r#"(?m)^[ \t]*(?:source|\.)[ \t]+(?:"([^"\r\n]+)"|'([^'\r\n]+)'|([^\s;#]+))"#
);
pattern!(
    POWERSHELL,
    r#"(?im)^[ \t]*(?:\.|Import-Module(?:[ \t]+-Name)?|using[ \t]+module)[ \t]+(?:"([^"\r\n]+)"|'([^'\r\n]+)'|([^\s;#]+))"#
);
pattern!(
    RUBY,
    r#"(?m)^[ \t]*require_relative[ (\t]*["']([^"'\r\n]+)["']"#
);
pattern!(JAVA_PACKAGE, r"(?m)^\s*package\s+([\w.]+)\s*;");
pattern!(JAVA_IMPORT, r"(?m)^\s*import\s+([\w.]+)\s*;");
pattern!(CS_NAMESPACE, r"(?m)^\s*namespace\s+([\w.]+)");
pattern!(
    CS_TYPE,
    r"\b(?:class|struct|interface|enum|record(?:\s+(?:class|struct))?)\s+(\w+)"
);
pattern!(
    CS_USING,
    r"(?m)^\s*(?:global\s+)?using\s+(?:(static\s+|\w+\s*=\s*))?(?:global::)?([\w.]+)\s*;"
);
pattern!(
    GO_IMPORT,
    r#"(?m)^[ \t]*import[ \t]+(?:[\w.]+[ \t]+)?["`]([^"`\r\n]+)["`]"#
);
pattern!(GO_BLOCK, r"(?ms)^[ \t]*import\s*\((.*?)\)");
pattern!(
    GO_ENTRY,
    r#"(?m)^[ \t]*(?:[\w.]+[ \t]+)?["`]([^"`\r\n]+)["`]"#
);

pub fn imports(path: &Path, bytes: &[u8], language: &str) -> Imports {
    let mut result = Imports::default();
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let text = String::from_utf8_lossy(bytes);
    let pattern = match ext.as_str() {
        "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hh" | "hxx" | "ipp" | "inl" => &*HEADER,
        "sh" | "bash" => &*BASH,
        "ps1" | "psm1" => &*POWERSHELL,
        "rb" => &*RUBY,
        "java" => {
            if let Some(package) = JAVA_PACKAGE.captures(&text) {
                result.declarations.push(format!(
                    "{}.{}",
                    &package[1],
                    path.file_stem().unwrap_or_default().to_string_lossy()
                ));
            }
            result
                .symbols
                .extend(JAVA_IMPORT.captures_iter(&text).map(|c| c[1].to_owned()));
            return result;
        }
        "cs" => {
            if let Some(namespace) = CS_NAMESPACE.captures(&text) {
                result.declarations.extend(
                    CS_TYPE
                        .captures_iter(&text)
                        .map(|c| format!("{}.{}", &namespace[1], &c[1])),
                );
            }
            for c in CS_USING.captures_iter(&text) {
                if c.get(1).is_some() {
                    result.symbols.push(c[2].to_owned());
                } else {
                    result.namespaces.push(c[2].to_owned());
                }
            }
            if !result.namespaces.is_empty() {
                result.words.extend(
                    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned),
                );
            }
            return result;
        }
        "go" => {
            result
                .packages
                .extend(GO_IMPORT.captures_iter(&text).map(|c| c[1].to_owned()));
            for block in GO_BLOCK.captures_iter(&text) {
                result
                    .packages
                    .extend(GO_ENTRY.captures_iter(&block[1]).map(|c| c[1].to_owned()));
            }
            return result;
        }
        "" if language.eq_ignore_ascii_case("bash") || language.eq_ignore_ascii_case("shell") => {
            &*BASH
        }
        "rs" => &*RUST,
        "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" => &*JS,
        "py" => &*PY,
        _ => return result,
    };
    let imports = &mut result.paths;
    if path.extension().is_some_and(|ext| ext == "rs") {
        imports.extend(CRATE_USE.captures_iter(&text).map(|c| c[1].to_owned()));
    }
    for captures in pattern.captures_iter(&text) {
        let module = captures.iter().skip(1).flatten().next().unwrap().as_str();
        if ext == "py"
            && let Some(names) = captures.get(2)
            && module.bytes().all(|b| b == b'.')
        {
            for name in names.as_str().trim_matches(['(', ')']).split(',') {
                if let Some(name) = name.split_whitespace().next() {
                    imports.push(format!("{module}{name}"));
                }
            }
        } else {
            let mut literal = module.to_owned();
            if matches!(ext.as_str(), "ps1" | "psm1") {
                literal = literal.replace('\\', "/");
                for prefix in ["$PSScriptRoot/", "${PSScriptRoot}/"] {
                    if literal
                        .get(..prefix.len())
                        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
                    {
                        literal = literal[prefix.len()..].to_owned();
                        break;
                    }
                }
            }
            if !literal.contains(['$', '`']) && !literal.contains("#{") {
                imports.push(literal);
            }
        }
    }
    result
}

fn candidates(source: &Path, import: &str, files: &HashSet<&Path>) -> Vec<PathBuf> {
    let parent = source.parent().unwrap_or(Path::new(""));
    let ext = source
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "rs" => {
            if let Some(name) = import.strip_prefix("crate::") {
                let Some(root) = parent.ancestors().find(|p| {
                    ["lib.rs", "main.rs"]
                        .iter()
                        .any(|f| files.contains(p.join(f).as_path()))
                }) else {
                    return Vec::new();
                };
                let module = root.join(name);
                return vec![module.with_extension("rs"), module.join("mod.rs")];
            }
            let mut base = parent.to_owned();
            if !matches!(
                source.file_stem().and_then(|f| f.to_str()),
                Some("main" | "lib" | "mod")
            ) {
                base.push(source.file_stem().unwrap_or_default());
            }
            let module = base.join(import);
            vec![module.with_extension("rs"), module.join("mod.rs")]
        }
        "py" => {
            let name = import.trim_start_matches('.');
            let mut base = parent.to_owned();
            for _ in 1..import.len() - name.len() {
                if !base.pop() {
                    return Vec::new();
                }
            }
            let module = base.join(name.replace('.', "/"));
            vec![module.with_extension("py"), module.join("__init__.py")]
        }
        _ => {
            let mut base = parent.to_owned();
            for component in Path::new(import).components() {
                match component {
                    Component::Normal(name) => base.push(name),
                    Component::CurDir => (),
                    Component::ParentDir if base.pop() => (),
                    _ => return Vec::new(),
                }
            }
            let mut paths = vec![base.clone()];
            if ext == "rb" {
                paths.push(PathBuf::from(format!("{}.rb", base.display())));
                return paths;
            }
            if matches!(
                ext.as_str(),
                "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" | "hh" | "hxx" | "ipp" | "inl"
            ) {
                paths.push(Path::new("include").join(import));
                paths.push(PathBuf::from(import));
                return paths;
            }
            if !matches!(ext.as_str(), "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs") {
                return paths;
            }
            for ext in ["js", "jsx", "ts", "tsx", "mjs", "cjs"] {
                paths.push(PathBuf::from(format!("{}.{ext}", base.display())));
                paths.push(base.join(format!("index.{ext}")));
            }
            paths
        }
    }
}

pub struct Resolution {
    pub references: BTreeMap<String, BTreeSet<String>>,
    pub importers: BTreeMap<String, BTreeSet<String>>,
    pub coverage: Value,
}

fn normalize(path: &Path) -> Option<PathBuf> {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => result.push(name),
            Component::CurDir => (),
            Component::ParentDir if result.pop() => (),
            _ => return None,
        }
    }
    Some(result)
}

fn alias_paths(config: &Value, directory: &Path, import: &str) -> Vec<PathBuf> {
    let base = directory.join(config["compilerOptions"]["baseUrl"].as_str().unwrap_or("."));
    let mut result = Vec::new();
    if let Some(paths) = config["compilerOptions"]["paths"].as_object() {
        for (pattern, targets) in paths {
            let capture = if let Some((start, end)) = pattern.split_once('*') {
                import.strip_prefix(start).and_then(|s| s.strip_suffix(end))
            } else {
                (pattern == import).then_some("")
            };
            if let Some(capture) = capture {
                for target in targets
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    let path = base.join(target.replace('*', capture));
                    if let Some(path) = normalize(&path) {
                        result.push(path.clone());
                        for ext in ["ts", "tsx", "js", "jsx", "mjs", "cjs"] {
                            result.push(PathBuf::from(format!("{}.{ext}", path.display())));
                            result.push(path.join(format!("index.{ext}")));
                        }
                    }
                }
            }
        }
    }
    result
}

pub fn resolve(root: &Path, scanned: &[(PathBuf, Lines)]) -> Resolution {
    let files: HashSet<_> = scanned.iter().map(|(p, _)| p.as_path()).collect();
    let display = |p: &Path| {
        p.components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    };
    let mut references = BTreeMap::<_, BTreeSet<_>>::new();
    let mut symbols = BTreeMap::<_, Vec<_>>::new();
    let mut packages = BTreeMap::<String, Vec<_>>::new();
    let mut modules = BTreeMap::new();
    let mut configs = BTreeMap::new();
    let mut cargo_roots = BTreeMap::new();
    let mut issues = BTreeSet::new();
    for (path, _) in scanned {
        let parent = path.parent().unwrap_or(Path::new(""));
        if path.file_name().is_some_and(|name| name == "tsconfig.json") {
            match std::fs::read_to_string(root.join(path))
                .ok()
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            {
                Some(config) => {
                    if config.get("extends").is_some() {
                        issues
                            .insert((display(path), "config extends requires fallback".to_owned()));
                    }
                    configs.insert(parent.to_owned(), config);
                }
                None => {
                    issues.insert((display(path), "could not parse tsconfig as JSON".to_owned()));
                }
            }
        }
        if path.file_name().is_some_and(|name| name == "Cargo.toml") {
            match std::fs::read_to_string(root.join(path))
                .ok()
                .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
            {
                Some(manifest) => {
                    let mut targets = vec![parent.join("src/lib.rs"), parent.join("src/main.rs")];
                    if let Some(path) = manifest
                        .get("lib")
                        .and_then(|v| v.get("path"))
                        .and_then(toml::Value::as_str)
                    {
                        targets.push(parent.join(path));
                    }
                    for section in ["bin", "test", "bench", "example"] {
                        for target in manifest
                            .get(section)
                            .and_then(toml::Value::as_array)
                            .into_iter()
                            .flatten()
                        {
                            if let Some(path) = target.get("path").and_then(toml::Value::as_str) {
                                targets.push(parent.join(path));
                            }
                        }
                    }
                    cargo_roots.insert(parent.to_owned(), targets);
                }
                None => {
                    issues.insert((display(path), "could not parse Cargo manifest".to_owned()));
                }
            }
        }
    }
    for (path, lines) in scanned {
        for name in &lines.imports.declarations {
            symbols
                .entry((path.extension(), name.as_str()))
                .or_default()
                .push(path);
        }
        if path.extension().is_some_and(|ext| ext == "go")
            && !path.to_string_lossy().ends_with("_test.go")
        {
            let parent = path.parent().unwrap_or(Path::new(""));
            for dir in parent.ancestors() {
                let module = modules.entry(dir).or_insert_with(|| {
                    let manifest = std::fs::read_to_string(root.join(dir).join("go.mod")).ok()?;
                    manifest
                        .lines()
                        .map(str::split_whitespace)
                        .find_map(|mut tokens| {
                            (tokens.next() == Some("module"))
                                .then(|| tokens.next())
                                .flatten()
                                .map(|s| s.trim_matches('"').to_owned())
                        })
                });
                if let Some(module) = module {
                    let sub = display(parent.strip_prefix(dir).unwrap());
                    let key = if sub.is_empty() {
                        module.clone()
                    } else {
                        format!("{module}/{sub}")
                    };
                    packages.entry(key).or_default().push(path);
                    break;
                }
            }
        }
    }
    for (source, lines) in scanned {
        let mut add = |target: &Path| {
            if target != source {
                references
                    .entry(display(source))
                    .or_default()
                    .insert(display(target));
            }
        };
        for package in &lines.imports.packages {
            for target in packages.get(package).into_iter().flatten() {
                add(target);
            }
        }
        for symbol in &lines.imports.symbols {
            for target in symbols
                .get(&(source.extension(), symbol.as_str()))
                .into_iter()
                .flatten()
            {
                add(target);
            }
        }
        for namespace in &lines.imports.namespaces {
            let prefix = format!("{namespace}.");
            for ((ext, name), targets) in symbols.range((source.extension(), prefix.as_str())..) {
                if *ext != source.extension() || !name.starts_with(&prefix) {
                    break;
                }
                if lines.imports.words.contains(&name[prefix.len()..]) {
                    for target in targets {
                        add(target);
                    }
                }
            }
        }
        for import in &lines.imports.paths {
            let ext = source.extension().and_then(|e| e.to_str()).unwrap_or("");
            let mut paths = candidates(source, import, &files);
            if matches!(ext, "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs")
                && !import.starts_with('.')
            {
                paths.clear();
                if let Some((dir, config)) = source
                    .parent()
                    .into_iter()
                    .flat_map(Path::ancestors)
                    .find_map(|dir| configs.get_key_value(dir))
                {
                    paths = alias_paths(config, dir, import);
                }
                if paths.is_empty() {
                    continue;
                }
            }
            if ext == "rs"
                && !import.starts_with("crate::")
                && let Some((_, targets)) = source
                    .parent()
                    .into_iter()
                    .flat_map(Path::ancestors)
                    .find_map(|dir| cargo_roots.get_key_value(dir))
                && targets.contains(source)
            {
                let dir = source.parent().unwrap_or(Path::new(""));
                paths = vec![
                    dir.join(format!("{import}.rs")),
                    dir.join(import).join("mod.rs"),
                ];
            }
            if ext == "rs"
                && let Some(name) = import.strip_prefix("crate::")
                && let Some((_, targets)) = source
                    .parent()
                    .into_iter()
                    .flat_map(Path::ancestors)
                    .find_map(|dir| cargo_roots.get_key_value(dir))
            {
                let target = targets
                    .iter()
                    .find(|p| *p == source)
                    .or_else(|| targets.iter().find(|p| files.contains(p.as_path())));
                paths = target
                    .and_then(|p| p.parent())
                    .map(|dir| {
                        vec![
                            dir.join(format!("{name}.rs")),
                            dir.join(name).join("mod.rs"),
                        ]
                    })
                    .unwrap_or_default();
            }
            if let Some(target) = paths.into_iter().find(|p| files.contains(p.as_path())) {
                add(&target);
            } else {
                issues.insert((
                    display(source),
                    format!("unresolved local reference {import}"),
                ));
            }
        }
    }
    let mut importers = BTreeMap::<_, BTreeSet<_>>::new();
    for (source, targets) in &references {
        for target in targets {
            importers
                .entry(target.clone())
                .or_default()
                .insert(source.clone());
        }
    }
    let issues: Vec<_> = issues
        .into_iter()
        .map(|(path, message)| json!({"path":path,"message":message}))
        .collect();
    Resolution {
        references,
        importers,
        coverage: json!({
            "status":"partial", "method":"lexical", "issues":issues,
            "limitations":["Dynamic imports, comments and literals, unsupported syntax, and project-specific resolution can cause missing or incorrect edges."]
        }),
    }
}
