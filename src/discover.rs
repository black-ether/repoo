use std::collections::HashSet;

use anyhow::{Context, Result, bail, ensure};
use clap::{Args, ValueEnum};
use serde_json::json;

use crate::{
    comments::Comments,
    github::Github,
    output::{Format, Input, Writer, record, repo_name},
    prefilter::discovery_stats,
    search::{Repository, Search, date},
};

pub const EXAMPLE: &str = "repoo discover 'keyword=pixle,lang=rust,size=100-300KB,comment>10,stars>10,after=20230429,feed=5,format=json'";
pub const KNOWN_REPO: &str = "repoo discover 'repo=chrislgarry/Apollo-11'";

#[derive(Args)]
#[command(after_help = format!("PowerShell examples:\n  {EXAMPLE}\n  {KNOWN_REPO}\n\nQuote the whole query. repo=owner/name inspects a known repo and enables tree stats by default. limit caps search candidates (default 20). feed caps successful matches after filtering. stats=true adds repository file count and byte-based code and prose shares."))]
pub struct Discover {
    /// Filters such as keyword=pixle,lang=rust,size=100-300KB,stars>10.
    query: String,
}

struct Query {
    repo: Option<String>,
    search: Search,
    keyword: Option<String>,
    stars_above: Option<u64>,
    comment_above: Option<f64>,
    feed: Option<usize>,
    stats: bool,
    format: Format,
}

impl Query {
    fn parse(query: &str) -> Result<Self> {
        let mut options = Self {
            repo: None,
            search: Search {
                limit: 20,
                ..Search::default()
            },
            keyword: None,
            stars_above: None,
            comment_above: None,
            feed: None,
            stats: false,
            format: Format::Table,
        };
        let mut seen = HashSet::new();
        for term in query.split(',') {
            let index = term
                .find(['=', '>'])
                .with_context(|| format!("expected a filter in {term:?}"))?;
            let (key, operand) = term.split_at(index);
            let key = key.trim();
            let value = operand[1..].trim();
            ensure!(!value.is_empty(), "empty value for {key}");
            ensure!(seen.insert(key), "duplicate discovery filter {key}");
            match (key, &operand[..1]) {
                ("repo", "=") => {
                    ensure!(repo_name(value), "expected repo=owner/name");
                    options.repo = Some(value.to_owned());
                }
                ("keyword", "=") => options.keyword = Some(value.to_owned()),
                ("lang", "=") => options.search.language = Some(value.to_owned()),
                ("size", "=") => {
                    let upper = value.to_ascii_uppercase();
                    let range = upper
                        .strip_suffix("KB")
                        .context("size must use KB, for example 100-300KB")?;
                    let (min, max) = range
                        .split_once('-')
                        .context("size must be a range, for example 100-300KB")?;
                    let min = min.trim().parse().context("invalid minimum size")?;
                    let max = max.trim().parse().context("invalid maximum size")?;
                    ensure!(min <= max, "minimum size must be at most maximum size");
                    options.search.min_size_kb = Some(min);
                    options.search.max_size_kb = Some(max);
                }
                ("comment", ">") => {
                    let pct = value
                        .trim_end_matches('%')
                        .parse()
                        .context("invalid comment percentage")?;
                    ensure!(
                        (0.0..=100.0).contains(&pct),
                        "comment percentage must be between 0 and 100"
                    );
                    options.comment_above = Some(pct);
                }
                ("stars", ">") => {
                    options.stars_above = Some(value.parse().context("invalid star count")?)
                }
                ("after", "=") => {
                    let formatted = if value.len() == 8 && value.bytes().all(|b| b.is_ascii_digit())
                    {
                        format!("{}-{}-{}", &value[..4], &value[4..6], &value[6..])
                    } else {
                        value.to_owned()
                    };
                    options.search.pushed_after =
                        Some(date(&formatted).map_err(anyhow::Error::msg)?);
                }
                ("format", "=") => {
                    options.format = Format::from_str(value, true).map_err(anyhow::Error::msg)?
                }
                ("limit", "=") => {
                    options.search.limit = value.parse().context("invalid candidate limit")?;
                    ensure!(
                        (1..=1000).contains(&options.search.limit),
                        "limit must be between 1 and 1000"
                    );
                }
                ("feed", "=") => {
                    let count = value.parse().context("invalid result limit for feed")?;
                    ensure!(count > 0, "feed must be a positive integer");
                    options.feed = Some(count);
                }
                ("stats", "=") => {
                    options.stats = value.parse().context("stats must be true or false")?
                }
                _ => bail!("unknown discovery filter {term:?}"),
            }
        }
        if options.repo.is_some() {
            ensure!(
                !["keyword", "lang", "size", "stars", "after", "limit"]
                    .iter()
                    .any(|key| seen.contains(key)),
                "repo= cannot be combined with keyword, lang, size, stars, after, or limit"
            );
            if !seen.contains("stats") {
                options.stats = true;
            }
        }
        Ok(options)
    }
}

impl Discover {
    pub fn run(self) -> Result<bool> {
        let query = Query::parse(&self.query)?;
        let comments = Comments::for_discovery();
        let mut writer = Writer::new(query.format)?;
        let mut partial = false;
        let mut matches = 0;
        let mut visit = |repo: Repository, github: &Github| -> Result<bool> {
            let metadata = serde_json::to_value(&repo)?;
            let mut report = if let Some(threshold) = query.comment_above {
                let mut report = comments.process(Input {
                    name: repo.full_name.clone(),
                    prefilter: None,
                });
                if report["error"].is_null()
                    && report["comment_pct"]
                        .as_f64()
                        .is_none_or(|pct| pct <= threshold)
                {
                    return Ok(true);
                }
                report["repo"] = json!(repo.full_name);
                for field in ["size_kb", "stars", "language", "pushed_at"] {
                    report[field] = metadata[field].clone();
                }
                report
            } else {
                metadata
            };
            if query.stats && report["error"].is_null() {
                match discovery_stats(github, &repo.full_name, &repo.default_branch) {
                    Ok(stats) => {
                        report["tree_files"] = stats["files"].clone();
                        for field in ["code_pct", "prose_pct", "truncated"] {
                            report[field] = stats[field].clone();
                        }
                    }
                    Err(error) => {
                        report["error"] =
                            json!({"code": "github_failed", "message": format!("{error:#}")})
                    }
                }
            }
            let failed = !report["error"].is_null();
            partial |= failed;
            writer.write(&report)?;
            matches += usize::from(!failed);
            Ok(query.feed != Some(matches))
        };
        let result = if let Some(slug) = &query.repo {
            let github = Github::new(false)?;
            let (owner, name) = slug.split_once('/').expect("validated slug");
            match github.get(&["repos", owner, name], &[]) {
                Ok(repo) => visit(repo, &github).map(|_| ()),
                Err(error) => {
                    partial = true;
                    writer.write(&record("prefilter", "repo", slug, "github_failed", || {
                        Err(error)
                    }))
                }
            }
        } else {
            query
                .search
                .visit(query.keyword.as_deref(), query.stars_above, visit)
        };
        writer.finish()?;
        result?;
        Ok(partial)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_discovery_query() {
        let query =
            Query::parse(EXAMPLE.split_once('\'').unwrap().1.trim_end_matches('\'')).unwrap();
        assert_eq!(query.keyword.as_deref(), Some("pixle"));
        assert_eq!(query.search.language.as_deref(), Some("rust"));
        assert_eq!(
            (query.search.min_size_kb, query.search.max_size_kb),
            (Some(100), Some(300))
        );
        assert_eq!(query.stars_above, Some(10));
        assert_eq!(query.comment_above, Some(10.0));
        assert_eq!(query.feed, Some(5));
        assert_eq!(query.search.pushed_after.as_deref(), Some("2023-04-29"));
        assert!(matches!(query.format, Format::Json));
        assert!(Query::parse("after=20230229").is_err());
        assert!(Query::parse("comment>NaN").is_err());
        assert!(Query::parse("feed=0").is_err());
        assert!(Query::parse("feed=-2").is_err());
        assert!(Query::parse("stats=true").unwrap().stats);
        assert!(Query::parse("stats=maybe").is_err());
        assert!(Query::parse("repo=chrislgarry/Apollo-11").unwrap().stats);
        assert!(
            !Query::parse("repo=chrislgarry/Apollo-11,stats=false,comment>10")
                .unwrap()
                .stats
        );
        assert!(Query::parse("repo=chrislgarry/Apollo-11,lang=rust").is_err());
        assert!(Query::parse("repo=Apollo-11").is_err());
    }
}
