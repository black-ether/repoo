use std::collections::BTreeMap;

use anyhow::{Result, ensure};
use clap::Args;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokei::LanguageType;

use crate::{
    comments::{GENERATED, MINIFIED, VENDOR, generated_file},
    github::Github,
    output::{Batch, percent, record, repo_name},
};

#[derive(Args)]
pub struct Prefilter {
    #[command(flatten)]
    batch: Batch,
    repos: Vec<String>,
    /// Additional case-insensitive glob over the repository-relative blob path.
    #[arg(long = "skip", value_name = "GLOB")]
    skip: Vec<String>,
    /// Apply only explicitly supplied --skip patterns.
    #[arg(long)]
    no_default_skip: bool,
    /// Report GitHub rate-limit headers on stderr.
    #[arg(long)]
    verbose: bool,
}

#[derive(Deserialize)]
struct Metadata {
    default_branch: String,
    size: u64,
    description: Option<String>,
}

#[derive(Deserialize)]
struct Tree {
    truncated: bool,
    tree: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(rename = "type")]
    kind: String,
    path: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Debug, PartialEq, Eq)]
enum Class {
    Code(&'static str),
    Prose,
    DataConfig,
    OtherBinary,
}

const PROSE: &[&str] = &[
    "md", "mdx", "rst", "txt", "adoc", "markdown", "asciidoc", "org",
];
const DATA: &[&str] = &[
    "json", "jsonc", "yaml", "yml", "toml", "ini", "cfg", "lock", "csv", "tsv", "xml",
];
const ASSETS: &[&str] = &["svg"];
const CODE_EXTENSIONS: &[(&str, &str)] = &[("agc", "Assembly")];
const CODE_NAMES: &[(&str, &str)] = &[
    ("dockerfile", "Dockerfile"),
    ("makefile", "Makefile"),
    ("cmakelists.txt", "CMake"),
];

fn classify(path: &str) -> Class {
    let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
    let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
    if parent.split('/').any(|directory| directory == "docs") {
        Class::Prose
    } else if let Some((_, language)) = CODE_NAMES.iter().find(|(file, _)| *file == name) {
        Class::Code(language)
    } else if PROSE.contains(&extension) {
        Class::Prose
    } else if DATA.contains(&extension) || crate::comments::LOCKS.contains(&name) {
        Class::DataConfig
    } else if ASSETS.contains(&extension) {
        Class::OtherBinary
    } else if let Some((_, language)) = CODE_EXTENSIONS
        .iter()
        .find(|(suffix, _)| *suffix == extension)
    {
        Class::Code(language)
    } else if let Some(language) = LanguageType::from_file_extension(extension) {
        Class::Code(language.name())
    } else {
        Class::OtherBinary
    }
}

fn default_skip(path: &str) -> bool {
    let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
    parent
        .split('/')
        .any(|part| VENDOR.contains(&part) || GENERATED.contains(&part))
        || generated_file(name)
        || MINIFIED.iter().any(|suffix| name.ends_with(suffix))
}

#[derive(Default, Serialize)]
struct Counts {
    bytes: u64,
    files: u64,
}

impl Counts {
    fn add(&mut self, size: u64) {
        self.bytes += size;
        self.files += 1;
    }
}

impl Tree {
    fn report(self, defaults: bool, skip: &GlobSet) -> Value {
        let (mut code, mut prose, mut data, mut other, mut total, mut skipped) = (
            Counts::default(),
            Counts::default(),
            Counts::default(),
            Counts::default(),
            Counts::default(),
            Counts::default(),
        );
        let mut languages = BTreeMap::<_, u64>::new();
        let mut included_code = 0;
        let mut included_prose = 0;
        for entry in self.tree.into_iter().filter(|entry| entry.kind == "blob") {
            let size = entry.size.unwrap_or_default();
            total.add(size);
            let path = entry.path.to_ascii_lowercase();
            let excluded = (defaults && default_skip(&path)) || skip.is_match(&entry.path);
            if excluded {
                skipped.add(size);
            }
            match classify(&path) {
                Class::Code(lang) => {
                    code.add(size);
                    if !excluded {
                        included_code += size;
                        *languages.entry(lang).or_default() += size;
                    }
                }
                Class::Prose => {
                    prose.add(size);
                    if !excluded {
                        included_prose += size;
                    }
                }
                Class::DataConfig => data.add(size),
                Class::OtherBinary => other.add(size),
            }
        }
        let mut languages: Vec<_> = languages.into_iter().collect();
        languages.sort_unstable_by(|(a_name, a_bytes), (b_name, b_bytes)| {
            b_bytes.cmp(a_bytes).then_with(|| a_name.cmp(b_name))
        });
        let top: Vec<_> = languages.into_iter().take(10).map(|(language, bytes)| {
            json!({"language": language, "bytes": bytes, "pct": percent(bytes, included_code)})
        }).collect();
        let included_bytes = total.bytes - skipped.bytes;
        let top_language = top
            .first()
            .map(|entry| json!({"language": entry["language"], "pct": entry["pct"]}));
        json!({
            "classes": {"code": code, "prose": prose, "data_config": data, "other_binary": other},
            "code_pct": percent(included_code, included_bytes),
            "prose_pct": percent(included_prose, included_bytes),
            "languages": top,
            "top_language": top_language,
            "files": total.files,
            "total_bytes": total.bytes,
            "skipped": skipped,
            "truncated": self.truncated,
        })
    }
}

impl Prefilter {
    pub fn run(self) -> Result<bool> {
        let mut builder = GlobSetBuilder::new();
        for pattern in &self.skip {
            builder.add(
                GlobBuilder::new(pattern)
                    .case_insensitive(true)
                    .literal_separator(true)
                    .build()?,
            );
        }
        let skip = builder.build()?;
        let github = Github::new(self.verbose)?;
        self.batch.run(&self.repos, "prefilter", false, |input| {
            if !repo_name(&input.name) {
                return record("prefilter", "repo", &input.name, "invalid_input", || {
                    anyhow::bail!("expected an owner/repo GitHub slug")
                });
            }
            record("prefilter", "repo", &input.name, "github_failed", || {
                let (owner, repo) = input.name.split_once('/').expect("validated slug");
                let meta: Metadata = github.get(&["repos", owner, repo], &[])?;
                let tree = fetch_tree(&github, &input.name, &meta.default_branch)?;
                let mut report = tree.report(!self.no_default_skip, &skip);
                report["size_kb"] = json!(meta.size);
                report["description"] = json!(meta.description);
                Ok(report)
            })
        })
    }
}

fn fetch_tree(github: &Github, slug: &str, branch: &str) -> Result<Tree> {
    ensure!(repo_name(slug), "expected an owner/repo GitHub slug");
    ensure!(
        !branch.is_empty(),
        "GitHub returned an empty default branch"
    );
    let (owner, repo) = slug.split_once('/').expect("validated slug");
    github.get(
        &["repos", owner, repo, "git", "trees", branch],
        &[("recursive", "1")],
    )
}

pub(crate) fn discovery_stats(github: &Github, slug: &str, branch: &str) -> Result<Value> {
    let metadata;
    let branch = if branch.is_empty() {
        ensure!(repo_name(slug), "expected an owner/repo GitHub slug");
        let (owner, repo) = slug.split_once('/').expect("validated slug");
        metadata = github.get::<Metadata>(&["repos", owner, repo], &[])?;
        &metadata.default_branch
    } else {
        branch
    };
    let tree = fetch_tree(github, slug, branch)?;
    Ok(tree.report(true, &GlobSet::empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_apollo_guidance_computer_assembly_as_code() {
        let tree: Tree = serde_json::from_value(json!({
            "truncated": false,
            "tree": [
                {"path": "Comanche055/MAIN.agc", "type": "blob", "size": 80},
                {"path": "README.md", "type": "blob", "size": 20}
            ]
        }))
        .unwrap();
        let report = tree.report(true, &GlobSet::empty());
        assert_eq!(report["code_pct"], 80.0);
        assert_eq!(report["prose_pct"], 20.0);
        assert_eq!(
            report["top_language"],
            json!({"language": "Assembly", "pct": 100.0})
        );
    }

    #[test]
    fn classifies_paths() {
        assert_eq!(classify("src/lib.rs"), Class::Code("Rust"));
        assert_eq!(classify("docs/examples/lib.rs"), Class::Prose);
        assert_eq!(classify("README.mdx"), Class::Prose);
        assert_eq!(classify("pnpm-lock.yaml"), Class::DataConfig);
        assert_eq!(classify("images/diagram.png"), Class::OtherBinary);
        assert!(default_skip("src/vendor/file.rs"));
        assert!(!default_skip("src/vendor.rs"));
    }

    #[test]
    fn parses_documented_github_response() {
        // Reduced from the Get a tree example in the GitHub REST API docs.
        let tree: Tree =
            serde_json::from_str(include_str!("../tests/fixtures/github-tree-example.json"))
                .unwrap();
        let value = tree.report(true, &GlobSetBuilder::new().build().unwrap());
        assert_eq!(value["files"], 2);
        assert_eq!(value["total_bytes"], 105);
        assert_eq!(value["languages"][0]["language"], "Ruby");
        assert_eq!(
            value["top_language"],
            json!({"language":"Ruby", "pct":100.0})
        );
        assert_eq!(value["truncated"], false);
    }

    #[test]
    fn parses_truncated_tree_response() {
        let tree: Tree = serde_json::from_str(include_str!("../tests/fixtures/tree.json")).unwrap();
        let value = tree.report(true, &GlobSetBuilder::new().build().unwrap());
        assert_eq!(value["truncated"], true);
        assert_eq!(value["files"], 4);
        assert_eq!(value["total_bytes"], 180);
        assert_eq!(value["skipped"]["bytes"], 80);
        assert_eq!(value["classes"]["code"]["bytes"], 130);
        assert_eq!(value["code_pct"], 50.0);
        assert_eq!(value["prose_pct"], 25.0);
        assert_eq!(value["languages"][0]["pct"], 100.0);
    }
}
