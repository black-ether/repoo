use std::{
    io::{self, BufRead, Seek, Write},
    process::{Command as ProcessCommand, ExitCode, Stdio},
    sync::mpsc,
};

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, Parser, Subcommand, ValueEnum};

use serde_json::{Value, json};

use crate::discover::{EXAMPLE as SCREEN, KNOWN_REPO};
const PIPE: &str =
    "repoo discover 'lang=rust,size=0-300KB,format=ndjson' | repoo prefilter --stdin";

#[derive(Parser)]
#[command(version, about, color = clap::ColorChoice::Never, after_help = format!(
    "Exit codes: 0 all ok, 1 fatal, 2 one or more repository errors\n\nInstall from source: cargo install --path . --locked\n\nExamples:\n  {KNOWN_REPO}\n  {SCREEN}\n  repoo analyze ."
))]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Discover GitHub repositories using a comma-separated query.
    Discover(crate::discover::Discover),
    /// Analyze local repositories by language, directory, and file.
    #[command(
        after_help = "Examples:\n  repoo analyze .\n  repoo analyze . --format json --select languages,largest_files"
    )]
    Analyze(crate::comments::Comments),
    /// Count lines in local repositories.
    #[command(hide = true, after_help = format!(
        "Exit codes: 0 all ok, 1 fatal, 2 repository errors\n\nExamples:\n  repoo comments ./checkout --format table\n  {SCREEN}\n  {PIPE}"
    ))]
    Comments(crate::comments::Comments),
    /// Estimate repository composition from GitHub tree metadata.
    #[command(hide = true, after_help = format!(
        "Exit codes: 0 all ok, 1 fatal, 2 repository errors\n\nExamples:\n  repoo prefilter BurntSushi/ripgrep --format json --select size_kb,top_language,description\n  {PIPE}"
    ))]
    Prefilter(crate::prefilter::Prefilter),
    /// Print the JSON Schema for an individual result record.
    #[command(after_help = format!(
        "Exit codes: 0 all ok, 1 fatal, 2 repository errors\n\nExamples:\n  repoo schema > schema.json\n  {PIPE}"
    ))]
    Schema,
}

pub fn entry() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let code = u8::from(error.use_stderr());
            let _ = error.print();
            return ExitCode::from(code);
        }
    };
    let result = match cli.command {
        Command::Discover(args) => args.run(),
        Command::Analyze(args) => args.run(true),
        Command::Comments(args) => args.run(false),
        Command::Prefilter(args) => args.run(),
        Command::Schema => io::stdout()
            .lock()
            .write_all(include_bytes!("../docs/schema.json"))
            .map(|()| false)
            .map_err(Into::into),
    };
    match result {
        Ok(partial) => ExitCode::from(if partial { 2 } else { 0 }),
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Clone, Copy, Default, ValueEnum)]
pub enum Format {
    Toon,
    Ndjson,
    Json,
    #[default]
    Table,
}

#[derive(Args, Default)]
pub struct Batch {
    /// Read names or NDJSON records from stdin.
    #[arg(long)]
    stdin: bool,
    #[arg(long, value_enum, default_value = "table")]
    format: Format,
    /// Comma-separated fields to keep alongside record identifiers and errors.
    #[arg(long, value_delimiter = ',', value_parser = clap::builder::NonEmptyStringValueParser::new())]
    select: Vec<String>,
    /// Maximum worker threads (prefilter defaults to 8, comments to available CPUs).
    #[arg(long, value_parser = clap::value_parser!(u16).range(1..=256))]
    concurrency: Option<u16>,
    /// Filter successful results using numeric comparisons joined by &&.
    #[arg(long = "where", visible_alias = "filter")]
    filter: Option<String>,
}

pub struct Input {
    pub name: String,
    pub prefilter: Option<Value>,
}

fn input(line: &str, phase: &str) -> Result<Input> {
    if line.trim_start().starts_with('{') {
        let value: Value = serde_json::from_str(line).context("invalid input JSON")?;
        let name = if phase == "comments" {
            value
                .get("path")
                .or_else(|| value.get("repo"))
                .or_else(|| value.get("full_name"))
        } else {
            value.get("full_name").or_else(|| value.get("repo"))
        };
        let name = name
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty())
            .context("expected a nonempty repository name or path")?
            .to_owned();
        let carries_prefilter = phase == "comments" && value["phase"] == "prefilter";
        if carries_prefilter {
            ensure!(
                value["repo"].as_str().is_some()
                    && value["schema"] == 1
                    && value.get("error").is_some(),
                "expected a schema v1 prefilter record"
            );
        }
        let prefilter = carries_prefilter.then_some(value);
        Ok(Input { name, prefilter })
    } else {
        Ok(Input {
            name: line.to_owned(),
            prefilter: None,
        })
    }
}

impl Batch {
    pub fn run(
        &self,
        args: &[String],
        phase: &str,
        details: bool,
        process: impl Fn(Input) -> Value + Sync,
    ) -> Result<bool> {
        ensure!(
            !args.is_empty() || self.stdin,
            "provide inputs or use --stdin"
        );
        ensure!(
            self.select.is_empty() || !matches!(self.format, Format::Table),
            "--select requires TOON, NDJSON, or JSON output"
        );
        let filters = self
            .filter
            .as_deref()
            .map(parse_filter)
            .transpose()?
            .unwrap_or_default();
        let default = if phase == "prefilter" {
            8
        } else {
            std::thread::available_parallelism().map_or(1, usize::from)
        };
        let threads = self.concurrency.map_or(default, usize::from);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .build()?;
        let mut writer = Writer::with_details(self.format, details)?;
        std::thread::scope(|scope| {
            let (tx, rx) = mpsc::sync_channel::<Result<Value>>(threads);
            let process = &process;
            scope.spawn(move || {
                let key = if phase == "comments" { "path" } else { "repo" };
                let (permit, available) = mpsc::sync_channel(threads);
                for _ in 0..threads {
                    permit.send(()).expect("permit receiver is alive");
                }
                pool.in_place_scope_fifo(|jobs| {
                    let submit = |item: Result<Input>, label: String| {
                        available.recv().expect("permit sender is alive");
                        let tx = tx.clone();
                        let permit = permit.clone();
                        jobs.spawn_fifo(move |_| {
                            let result = match item {
                                Ok(item) => process(item),
                                Err(error) => {
                                    record(phase, key, &label, "invalid_input", || Err(error))
                                }
                            };
                            let _ = tx.send(Ok(result));
                            let _ = permit.send(());
                        });
                    };
                    for name in args {
                        submit(
                            Ok(Input {
                                name: name.clone(),
                                prefilter: None,
                            }),
                            String::new(),
                        );
                    }
                    if self.stdin {
                        for (index, line) in io::stdin().lock().lines().enumerate() {
                            let label = format!("stdin line {}", index + 1);
                            match line {
                                Ok(line) if !line.trim().is_empty() => {
                                    submit(input(line.trim(), phase), label)
                                }
                                Ok(_) => {}
                                Err(error) => {
                                    let _ = tx.send(Err(error.into()));
                                    break;
                                }
                            }
                        }
                    }
                });
            });
            let mut partial = false;
            for value in rx {
                let mut value = match value {
                    Ok(value) => value,
                    Err(error) => {
                        writer.finish()?;
                        return Err(error);
                    }
                };
                let failed = !value["error"].is_null();
                partial |= failed;
                if failed || filters.iter().all(|filter| filter.matches(&value)) {
                    if !self.select.is_empty() {
                        value
                            .as_object_mut()
                            .expect("record object")
                            .retain(|key, _| {
                                matches!(
                                    key.as_str(),
                                    "schema" | "phase" | "repo" | "path" | "error"
                                ) || self.select.contains(key)
                            });
                    }
                    writer.write(&value)?;
                }
            }
            writer.finish()?;
            Ok(partial)
        })
    }
}

pub fn record(
    phase: &str,
    key: &str,
    name: &str,
    code: &str,
    compute: impl FnOnce() -> Result<Value>,
) -> Value {
    let mut result = match compute() {
        Ok(mut metrics) => {
            metrics["error"] = Value::Null;
            metrics
        }
        Err(error) => json!({"error": {"code": code, "message": format!("{error:#}")}}),
    };
    result["schema"] = json!(1);
    result["phase"] = json!(phase);
    if result.get(key).is_none() {
        result[key] = json!(name);
    }
    result
}

pub fn repo_name(name: &str) -> bool {
    name.split_once('/').is_some_and(|(owner, repo)| {
        [owner, repo].into_iter().all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.ends_with('.')
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
    })
}

pub fn percent(numerator: u64, denominator: u64) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        100.0 * numerator as f64 / denominator as f64
    }
}

pub struct Writer {
    format: Format,
    stdout: io::StdoutLock<'static>,
    first: bool,
    toon_rows: Vec<Value>,
    details: bool,
}

impl Writer {
    pub fn new(format: Format) -> Result<Self> {
        Self::with_details(format, false)
    }

    fn with_details(format: Format, details: bool) -> Result<Self> {
        let mut writer = Self {
            format,
            stdout: io::stdout().lock(),
            first: true,
            toon_rows: Vec::new(),
            details,
        };
        if matches!(format, Format::Json) {
            write!(writer.stdout, "[")?;
        }
        if matches!(format, Format::Table) && !details {
            writeln!(writer.stdout, "REPOSITORY\tDETAILS")?;
        }
        Ok(writer)
    }

    pub fn write(&mut self, value: &Value) -> Result<()> {
        match self.format {
            Format::Toon => self.toon_rows.push(value.clone()),
            Format::Ndjson => {
                serde_json::to_writer(&mut self.stdout, value)?;
                writeln!(self.stdout)?;
                self.stdout.flush()?;
            }
            Format::Json => {
                if !self.first {
                    write!(self.stdout, ",")?;
                }
                serde_json::to_writer(&mut self.stdout, value)?;
            }
            Format::Table if self.details => {
                if !self.first {
                    writeln!(self.stdout)?;
                }
                write_analysis(&mut self.stdout, value)?;
            }
            Format::Table => {
                let name = value
                    .get("size_kb")
                    .and_then(|_| value["repo"].as_str())
                    .or(value["path"].as_str())
                    .or(value["repo"].as_str())
                    .or(value["full_name"].as_str())
                    .unwrap_or("");
                let phase = value["phase"].as_str().unwrap_or("search");
                let summary = if let Some(message) = value["error"]["message"].as_str() {
                    format!("error: {message:?}")
                } else if phase == "comments" {
                    let discovery = value.get("size_kb").map_or_else(String::new, |size| {
                        format!("{size} KB, {} stars | ", value["stars"])
                    });
                    format!(
                        "{discovery}{} files, {} code, {} comments, {:.2}% comments",
                        value["files"],
                        value["code"],
                        value["comments"],
                        value["comment_pct"].as_f64().unwrap_or_default()
                    )
                } else if phase == "prefilter" {
                    let top = &value["top_language"];
                    let language = match (top["language"].as_str(), top["pct"].as_f64()) {
                        (Some(name), Some(pct)) => format!("{name} {pct:.0}%"),
                        _ => "—".to_owned(),
                    };
                    let description = value["description"].as_str().unwrap_or("—");
                    format!(
                        "{} KB | {} | {} files, {:.2}% code, {:.2}% prose{} | {}",
                        value["size_kb"],
                        language,
                        value["files"],
                        value["code_pct"].as_f64().unwrap_or_default(),
                        value["prose_pct"].as_f64().unwrap_or_default(),
                        if value["truncated"] == true {
                            " (truncated tree)"
                        } else {
                            ""
                        },
                        description.split_whitespace().collect::<Vec<_>>().join(" ")
                    )
                } else {
                    format!(
                        "{} KB, {} stars, {}, {}",
                        value["size_kb"], value["stars"], value["language"], value["pushed_at"]
                    )
                };
                write!(self.stdout, "{}\t{summary}", name.escape_debug())?;
                if let Some(files) = value["tree_files"].as_u64() {
                    write!(
                        self.stdout,
                        " | {files} repo files, {:.2}% code, {:.2}% prose{}",
                        value["code_pct"].as_f64().unwrap_or_default(),
                        value["prose_pct"].as_f64().unwrap_or_default(),
                        if value["truncated"] == true {
                            " (truncated tree)"
                        } else {
                            ""
                        }
                    )?;
                }
                writeln!(self.stdout)?;
            }
        }
        self.first = false;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        if matches!(self.format, Format::Toon) {
            let mut input = tempfile::tempfile()?;
            serde_json::to_writer(&mut input, &self.toon_rows)?;
            input.rewind()?;
            let program = if cfg!(windows) { "toon.cmd" } else { "toon" };
            let status = ProcessCommand::new(program)
                .arg("--encode")
                .stdin(Stdio::from(input))
                .status()
                .context("could not launch TOON encoder; install it with npm install -g @toon-format/cli or use --format ndjson")?;
            ensure!(status.success(), "TOON encoder exited with {status}");
        }
        if matches!(self.format, Format::Json) {
            writeln!(self.stdout, "]")?;
        }
        self.stdout.flush()?;
        Ok(())
    }
}

fn write_analysis(out: &mut impl Write, value: &Value) -> Result<()> {
    let path = value["path"]
        .as_str()
        .unwrap_or("")
        .escape_debug()
        .to_string();
    if let Some(message) = value["error"]["message"].as_str() {
        writeln!(out, "repo: {path}\n  Error: {message:?}")?;
        return Ok(());
    }
    let mut languages: Vec<_> = value["languages"]
        .as_object()
        .into_iter()
        .flatten()
        .collect();
    languages.sort_by_key(|(name, lines)| {
        (
            std::cmp::Reverse(lines["code"].as_u64().unwrap_or_default()),
            *name,
        )
    });
    let shares = languages
        .into_iter()
        .take(5)
        .map(|(name, lines)| format!("{}L {name}", lines["code"]))
        .collect::<Vec<_>>()
        .join(" · ");
    let inventoried = value["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|group| group["files"].as_u64())
        .sum::<u64>()
        + value["unclassified_files"].as_u64().unwrap_or_default();
    let total_bytes = value["tree"]["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| !entry["path"].as_str().unwrap_or("").contains('/'))
        .filter_map(|entry| entry["bytes"].as_u64())
        .sum::<u64>();
    writeln!(
        out,
        "repo: {path} ({inventoried} files · {} total · {shares})",
        crate::tree::size(total_bytes)
    )?;
    if value.get("tree").is_some() {
        crate::tree::render(out, &value["tree"])?;
    }
    writeln!(out, "\nENTRY & HUBS")?;
    let graph = value["references"].as_object();
    let groups = value["groups"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let entries: Vec<_> = groups
        .iter()
        .flat_map(|group| group["key_files"].as_array().into_iter().flatten())
        .filter(|file| file["reason"] == "entry point")
        .filter_map(|file| file["path"].as_str())
        .collect();
    for entry in &entries {
        let targets = graph
            .and_then(|g| g.get(*entry))
            .and_then(Value::as_array)
            .map(|targets| {
                targets
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|target| {
                        let source = std::path::Path::new(entry);
                        let target_path = std::path::Path::new(target);
                        if source.parent() == target_path.parent() {
                            target_path
                                .file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        } else {
                            target.to_owned()
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        writeln!(
            out,
            "  {entry} (entry){}",
            if targets.is_empty() {
                String::new()
            } else {
                format!(" ──▶ {targets}")
            }
        )?;
    }
    let mut hubs: Vec<_> = value["importers"]
        .as_object()
        .into_iter()
        .flatten()
        .collect();
    hubs.sort_by_key(|(path, sources)| {
        (
            std::cmp::Reverse(sources.as_array().map_or(0, Vec::len)),
            *path,
        )
    });
    let hubs = hubs
        .into_iter()
        .take(4)
        .map(|(path, sources)| {
            format!(
                "{} ({})",
                path.escape_debug(),
                sources.as_array().map_or(0, Vec::len)
            )
        })
        .collect::<Vec<_>>()
        .join(" · ");
    if !hubs.is_empty() {
        writeln!(out, "  hubs (by incoming refs): {hubs}")?;
    }
    let edges = graph
        .into_iter()
        .flatten()
        .filter(|(source, _)| !entries.contains(&source.as_str()))
        .flat_map(|(source, targets)| {
            targets
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(move |target| {
                    format!("{} ──▶ {}", source.escape_debug(), target.escape_debug())
                })
        })
        .take(3)
        .collect::<Vec<_>>()
        .join(" · ");
    if !edges.is_empty() {
        writeln!(out, "  key edges: {edges}")?;
    }
    let issues = value["reference_coverage"]["issues"]
        .as_array()
        .map_or(0, Vec::len);
    writeln!(
        out,
        "  coverage: partial (lexical scan, {issues} reported issues)"
    )?;

    if let Some(files) = value["largest_code_files"]
        .as_array()
        .filter(|f| !f.is_empty())
    {
        writeln!(out, "\nLARGEST CODE FILES")?;
        let width = files
            .iter()
            .take(4)
            .filter_map(|f| f["path"].as_str())
            .map(str::len)
            .max()
            .unwrap_or_default();
        for file in files.iter().take(4) {
            let path = file["path"]
                .as_str()
                .unwrap_or("")
                .escape_debug()
                .to_string();
            let comments = file["comments"].as_u64().unwrap_or_default();
            writeln!(
                out,
                "  {path:<width$}  {}L ({comments} {})",
                file["code"],
                if comments == 1 { "comment" } else { "comments" }
            )?;
        }
    }
    writeln!(out, "\nCONFIG, DOCS & TESTS")?;
    for (name, label) in [
        ("config_build", "config"),
        ("docs_assets", "docs"),
        ("tests", "tests"),
    ] {
        let mut paths = groups
            .iter()
            .filter(|g| g["group"] == name)
            .flat_map(|g| g["key_files"].as_array().into_iter().flatten())
            .map(|file| {
                let path = file["path"]
                    .as_str()
                    .unwrap_or("")
                    .escape_debug()
                    .to_string();
                let code = file["code"].as_u64().unwrap_or_default();
                if name == "docs_assets" && code > 10 {
                    format!("{path} ({code}L)")
                } else {
                    path
                }
            })
            .collect::<Vec<_>>();
        if name == "tests" {
            let fixtures = value["fixture_directories"]
                .as_object()
                .into_iter()
                .flatten()
                .take(3)
                .map(|(path, count)| format!("{} ({count} files)", path.escape_debug()));
            paths.extend(fixtures);
        }
        if !paths.is_empty() {
            let label = format!("{label}:");
            writeln!(out, "  {label:<7} {}", paths.join(" · "))?;
        }
    }
    Ok(())
}

struct Filter<'a> {
    field: &'a str,
    op: &'a str,
    threshold: f64,
}

impl Filter<'_> {
    fn matches(&self, value: &Value) -> bool {
        let Some(actual) = value
            .get(self.field)
            .or_else(|| value["prefilter"].get(self.field))
            .and_then(Value::as_f64)
        else {
            return false;
        };
        match self.op {
            "<" => actual < self.threshold,
            "<=" => actual <= self.threshold,
            ">" => actual > self.threshold,
            ">=" => actual >= self.threshold,
            "==" => actual == self.threshold,
            "!=" => actual != self.threshold,
            _ => unreachable!(),
        }
    }
}

fn parse_filter(expression: &str) -> Result<Vec<Filter<'_>>> {
    expression
        .split("&&")
        .map(|part| {
            for op in ["<=", ">=", "==", "!=", "<", ">"] {
                if let Some((field, threshold)) = part.split_once(op) {
                    let field = field.trim();
                    let field = if field == "code_lines" {
                        "total_code_lines"
                    } else {
                        field
                    };
                    ensure!(
                        matches!(
                            field,
                            "code"
                                | "total_code_lines"
                                | "comments"
                                | "blanks"
                                | "files"
                                | "comment_pct"
                                | "code_pct"
                                | "prose_pct"
                                | "total_bytes"
                        ),
                        "unknown filter field {field:?}"
                    );
                    let threshold: f64 =
                        threshold.trim().parse().context("invalid filter number")?;
                    ensure!(threshold.is_finite(), "filter number must be finite");
                    return Ok(Filter {
                        field,
                        op,
                        threshold,
                    });
                }
            }
            bail!("expected a numeric comparison in {part:?}");
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::percent;

    #[test]
    fn calculates_percentages() {
        assert_eq!(percent(0, 0), 0.0);
        assert_eq!(percent(0, 3), 0.0);
        assert_eq!(percent(1, 4), 25.0);
        assert_eq!(percent(3, 3), 100.0);
    }
}
