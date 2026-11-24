use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{Context, Result, bail, ensure};
use clap::Args;
use encoding_rs_io::DecodeReaderBytesBuilder;
use ignore::WalkBuilder;
use rayon::prelude::*;
use serde::Serialize;
use serde_json::{Value, json};
use tokei::{Config, LanguageType};

use crate::output::{Batch, Input, percent, record, repo_name};

#[derive(Args, Default)]
pub struct Comments {
    #[command(flatten)]
    batch: Batch,
    paths: Vec<String>,
    /// Count vendor and dependency directories where ignore rules allow.
    #[arg(long)]
    include_vendor: bool,
    /// Count generated files where ignore rules allow.
    #[arg(long)]
    include_generated: bool,
    /// Count minified files where ignore rules allow.
    #[arg(long)]
    include_minified: bool,
    /// Allow binary-looking files with a Tokei-recognized language.
    #[arg(long)]
    include_binary: bool,
    #[arg(long)]
    include_ignored: bool,
    /// Clone missing GitHub slugs using Git.
    #[arg(long)]
    clone_missing: bool,
    #[arg(long, default_value = ".reporeport-cache")]
    cache_dir: PathBuf,
    /// Levels of children below root entries (0 shows root entries only).
    #[arg(long, default_value_t = 1)]
    tree_depth: u16,
    /// Hide sizes and scan only the displayed tree levels.
    #[arg(long)]
    no_tree_sizes: bool,
    /// Skip the tree and total-size scan.
    #[arg(long)]
    no_tree: bool,
}

pub const VENDOR: &[&str] = &["vendor", "node_modules", "third_party", "third-party"];
pub const GENERATED: &[&str] = &["dist", "build", "target", "generated", "__pycache__"];
pub const MINIFIED: &[&str] = &[".min.js", ".min.css"];
pub const LOCKS: &[&str] = &[
    "package-lock.json",
    "npm-shrinkwrap.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "cargo.lock",
    "composer.lock",
    "gemfile.lock",
    "poetry.lock",
    "uv.lock",
    "bun.lockb",
];

pub fn generated_file(name: &str) -> bool {
    LOCKS.contains(&name)
        || [".lock", ".generated.rs", ".generated.ts", ".pb.go"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

#[derive(Default, Serialize)]
pub(crate) struct Lines {
    #[serde(skip)]
    pub(crate) imports: crate::references::Imports,
    pub(crate) code: usize,
    pub(crate) comments: usize,
    pub(crate) blanks: usize,
    pub(crate) files: usize,
}

impl Lines {
    fn add(&mut self, other: &Self) {
        self.code += other.code;
        self.comments += other.comments;
        self.blanks += other.blanks;
        self.files += other.files;
    }
}

impl Comments {
    pub(crate) fn for_discovery() -> Self {
        Self {
            clone_missing: true,
            cache_dir: PathBuf::from(".reporeport-cache"),
            ..Self::default()
        }
    }

    pub fn run(self, details: bool) -> Result<bool> {
        self.batch.run(&self.paths, "comments", details, |input| {
            self.process_with_details(input, details)
        })
    }

    pub(crate) fn process(&self, input: Input) -> Value {
        self.process_with_details(input, false)
    }

    fn process_with_details(&self, input: Input, details: bool) -> Value {
        let source = input.prefilter;
        let requested = PathBuf::from(&input.name);
        let mut result = record("comments", "path", &input.name, "count_failed", || {
            let (path, mut metrics) =
                if !requested.exists() && self.clone_missing && repo_name(&input.name) {
                    let path = cached_clone(&self.cache_dir, &input.name)?;
                    let metrics = self.cached_count(&path, details)?;
                    (path, metrics)
                } else {
                    let metrics = self.count(&requested, details)?;
                    (requested, metrics)
                };
            metrics["path"] = json!(path);
            if details && !self.no_tree {
                metrics["tree"] =
                    crate::tree::scan(&path, usize::from(self.tree_depth) + 1, !self.no_tree_sizes);
            }
            Ok(metrics)
        });
        if let Some(source) = source {
            result["repo"] = source["repo"].clone();
            result["prefilter"] = source;
        }
        result
    }

    fn cached_count(&self, path: &Path, details: bool) -> Result<Value> {
        let flags = [
            self.include_vendor,
            self.include_generated,
            self.include_minified,
            self.include_binary,
            self.include_ignored,
        ];
        let options: String = flags
            .into_iter()
            .map(|flag| if flag { '1' } else { '0' })
            .collect();
        let sha = path
            .file_name()
            .expect("cache directory ends in the commit SHA")
            .to_string_lossy();
        let metrics_file = path.with_file_name(format!(
            "{sha}-{}-{options}{}.json",
            env!("CARGO_PKG_VERSION"),
            if details { "-analysis-v6" } else { "" }
        ));
        if let Some(metrics) = fs::read(&metrics_file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .filter(|metrics| metrics.get("total_code_lines").is_some())
        {
            return Ok(metrics);
        }
        let metrics = self.count(path, details)?;
        let parent = metrics_file
            .parent()
            .expect("metrics directory has a parent");
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer(&mut temp, &metrics)?;
        temp.persist(metrics_file)?;
        Ok(metrics)
    }

    fn count(&self, path: &Path, details: bool) -> Result<Value> {
        ensure!(
            path.is_dir(),
            "directory does not exist: {}",
            path.display()
        );
        let mut walker = WalkBuilder::new(path);
        walker
            .hidden(false)
            .parents(false)
            .require_git(false)
            .git_global(false);
        if self.include_ignored {
            walker.ignore(false).git_ignore(false).git_exclude(false);
        }
        walker.filter_entry({
            let (vendor, generated, minified) = (
                self.include_vendor,
                self.include_generated,
                self.include_minified,
            );
            move |entry| {
                if entry.depth() == 0 {
                    return true;
                }
                let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                if details {
                    return name != ".git";
                }
                if entry.file_type().is_some_and(|kind| kind.is_dir()) {
                    return name != ".git"
                        && (vendor || !VENDOR.contains(&name.as_str()))
                        && (generated || !GENERATED.contains(&name.as_str()));
                }
                name != ".git"
                    && (generated || !generated_file(&name))
                    && (minified || !MINIFIED.iter().any(|suffix| name.ends_with(suffix)))
            }
        });
        let config = Config::default();
        let (languages, scanned): (BTreeMap<_, Lines>, Vec<_>) = walker
            .build()
            .par_bridge()
            .try_fold(
                Default::default,
                |(mut languages, mut scanned): (BTreeMap<_, Lines>, Vec<_>), entry| {
                    let entry = entry?;
                    if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                        return Ok((languages, scanned));
                    }
                    let relative = entry.path().strip_prefix(path)?;
                    let excluded = details && self.excluded_from_count(relative);
                    let counted = if excluded {
                        None
                    } else {
                        count_file(entry.path(), &config, self.include_binary, details)
                            .with_context(|| format!("reading {}", entry.path().display()))?
                    };
                    if let Some((language, lines)) = counted {
                        languages
                            .entry(language)
                            .or_insert_with(Lines::default)
                            .add(&lines);
                        if details {
                            scanned.push((relative.to_owned(), lines));
                        }
                    } else if details {
                        scanned.push((relative.to_owned(), Lines::default()));
                    }
                    Ok::<_, anyhow::Error>((languages, scanned))
                },
            )
            .try_reduce(
                Default::default,
                |(mut left, mut scanned), (right, more)| {
                    for (language, lines) in right {
                        left.entry(language).or_default().add(&lines);
                    }
                    scanned.extend(more);
                    Ok((left, scanned))
                },
            )?;
        let mut totals = Lines::default();
        for lines in languages.values() {
            totals.add(lines);
        }
        let languages: BTreeMap<_, _> = languages
            .into_iter()
            .map(|(language, lines)| (language.name(), lines))
            .collect();
        let mut result = json!({
            "code": totals.code,
            "total_code_lines": totals.code,
            "comments": totals.comments,
            "blanks": totals.blanks,
            "files": totals.files,
            "comment_pct": percent(totals.comments as u64, (totals.code + totals.comments) as u64),
            "languages": languages,
        });
        if details {
            let (groups, unknown) = crate::inventory::summarize(&scanned);
            result["groups"] = groups;
            let mut fixtures = BTreeMap::<String, usize>::new();
            for (file, _) in &scanned {
                let relative = file.to_string_lossy().replace('\\', "/");
                let components: Vec<_> = relative.split('/').collect();
                if let Some(index) = components
                    .iter()
                    .position(|p| matches!(*p, "fixtures" | "testdata" | "__fixtures__"))
                {
                    *fixtures
                        .entry(format!("{}/", components[..=index].join("/")))
                        .or_default() += 1;
                }
            }
            result["fixture_directories"] = json!(fixtures);
            result["unclassified_files"] = json!(unknown);
            let scanned: Vec<_> = scanned
                .into_iter()
                .filter(|(_, lines)| lines.files > 0)
                .collect();
            for (language, lines) in languages {
                result["languages"][language]["share_pct"] =
                    json!(percent(lines.code as u64, totals.code as u64));
            }
            let mut directories = BTreeMap::<PathBuf, Lines>::new();
            let references = crate::references::resolve(path, &scanned);
            result["references"] = json!(references.references);
            result["importers"] = json!(references.importers);
            result["reference_coverage"] = references.coverage;
            for (path, lines) in &scanned {
                if path.components().count() > 1 {
                    let root = path.components().next().expect("relative path component");
                    directories
                        .entry(PathBuf::from(root.as_os_str()))
                        .or_default()
                        .add(lines);
                }
            }
            result["largest_directories"] =
                json!(largest(directories.into_iter().collect(), totals.code));
            let (code_files, other_files): (Vec<_>, Vec<_>) =
                scanned.into_iter().partition(|(path, _)| {
                    crate::inventory::is_code(&path.to_string_lossy().replace('\\', "/"))
                });
            let code_files = largest(code_files, totals.code);
            result["largest_code_files"] = json!(code_files);
            // Retain the original all-file ranking for structured consumers.
            let mut all_files = code_files;
            all_files.extend(largest(other_files, totals.code));
            all_files
                .sort_by_key(|row| std::cmp::Reverse(row["code"].as_u64().unwrap_or_default()));
            all_files.truncate(5);
            result["largest_files"] = json!(all_files);
        }
        Ok(result)
    }

    fn excluded_from_count(&self, path: &Path) -> bool {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_ascii_lowercase();
        path.parent()
            .into_iter()
            .flat_map(Path::components)
            .any(|part| {
                let name = part.as_os_str().to_string_lossy().to_ascii_lowercase();
                (!self.include_vendor && VENDOR.contains(&name.as_str()))
                    || (!self.include_generated && GENERATED.contains(&name.as_str()))
            })
            || (!self.include_generated && generated_file(&name))
            || (!self.include_minified && MINIFIED.iter().any(|s| name.ends_with(s)))
    }
}

fn largest(mut entries: Vec<(PathBuf, Lines)>, total: usize) -> Vec<Value> {
    entries.sort_unstable_by(|(a_path, a), (b_path, b)| {
        b.code.cmp(&a.code).then_with(|| a_path.cmp(b_path))
    });
    entries
        .into_iter()
        .take(5)
        .map(|(path, lines)| {
            json!({
            "path": path.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"),
                "code": lines.code,
                "comments": lines.comments,
                "blanks": lines.blanks,
                "files": lines.files,
                "share_pct": percent(lines.code as u64, total as u64),
            })
        })
        .collect()
}

fn count_file(
    path: &Path,
    config: &Config,
    include_binary: bool,
    details: bool,
) -> Result<Option<(LanguageType, Lines)>> {
    let Some(language) = LanguageType::from_path(path, config) else {
        return Ok(None);
    };
    let mut text = fs::read(path)?;
    if text.starts_with(&[0xff, 0xfe])
        || text.starts_with(&[0xfe, 0xff])
        || text.starts_with(&[0xef, 0xbb, 0xbf])
    {
        let mut decoded = Vec::new();
        DecodeReaderBytesBuilder::new()
            .strip_bom(true)
            .build(text.as_slice())
            .read_to_end(&mut decoded)?;
        text = decoded;
    }
    if !include_binary && text.contains(&0) {
        return Ok(None);
    }
    let stats = language.parse_from_slice(&text, config).summarise();
    Ok(Some((
        language,
        Lines {
            imports: if details {
                crate::references::imports(path, &text, language.name())
            } else {
                Default::default()
            },
            code: stats.code,
            comments: stats.comments,
            blanks: stats.blanks,
            files: 1,
        },
    )))
}

fn git(args: &[&str], directory: Option<&Path>) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null());
    if let Some(directory) = directory {
        cmd.current_dir(directory);
    }
    let result = cmd.output().context("could not run git")?;
    if !result.status.success() {
        bail!(
            "git {} failed: {}",
            args[0],
            String::from_utf8_lossy(&result.stderr).trim()
        );
    }
    Ok(String::from_utf8(result.stdout)?.trim().to_owned())
}

fn cached_clone(cache: &Path, repo: &str) -> Result<PathBuf> {
    let base = cache.join(repo.to_ascii_lowercase());
    fs::create_dir_all(&base)?;
    let marker = base.join("current");
    if let Ok(sha) = fs::read_to_string(&marker) {
        let sha = sha.trim();
        if valid_sha(sha) && base.join(sha).join(".git").is_dir() {
            return Ok(base.join(sha));
        }
    }
    let staging = tempfile::tempdir_in(&base)?;
    let url = format!("https://github.com/{repo}.git");
    git(
        &["clone", "--depth", "1", "--single-branch", "--", &url, "."],
        Some(staging.path()),
    )?;
    let sha = git(&["rev-parse", "HEAD"], Some(staging.path()))?;
    ensure!(valid_sha(&sha), "git returned an invalid commit SHA");
    let destination = base.join(&sha);
    if let Err(error) = fs::rename(staging.path(), &destination)
        && !destination.join(".git").is_dir()
    {
        return Err(error.into());
    }
    let mut marker_temp = tempfile::NamedTempFile::new_in(base)?;
    marker_temp.write_all(sha.as_bytes())?;
    marker_temp.persist(marker)?;
    Ok(destination)
}

fn valid_sha(sha: &str) -> bool {
    matches!(sha.len(), 40 | 64) && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
}
