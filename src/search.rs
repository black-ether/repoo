use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::github::Github;

#[derive(Default)]
pub struct Search {
    pub(crate) language: Option<String>,
    /// Maximum GitHub repository size in KB.
    pub(crate) max_size_kb: Option<u64>,
    /// Minimum GitHub repository size in KB.
    pub(crate) min_size_kb: Option<u64>,
    /// Earliest push date, exclusive (YYYY-MM-DD).
    pub(crate) pushed_after: Option<String>,
    pub(crate) limit: u16,
}

#[derive(Deserialize)]
struct Response {
    items: Vec<Repository>,
    incomplete_results: bool,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Repository {
    pub(crate) full_name: String,
    #[serde(skip_serializing, default)]
    pub(crate) default_branch: String,
    #[serde(rename(deserialize = "size"))]
    size_kb: u64,
    #[serde(rename(deserialize = "stargazers_count"))]
    stars: u64,
    pushed_at: Option<String>,
    language: Option<String>,
}

impl Search {
    pub(crate) fn visit(
        &self,
        keyword: Option<&str>,
        stars_above: Option<u64>,
        mut visit: impl FnMut(Repository, &Github) -> Result<bool>,
    ) -> Result<()> {
        let mut terms = Vec::new();
        if let Some(keyword) = keyword {
            ensure!(
                !keyword.contains(['"', '\\']) && !keyword.chars().any(char::is_control),
                "invalid GitHub keyword"
            );
            terms.push(format!("\"{keyword}\""));
        }
        if let Some(language) = &self.language {
            ensure!(
                !language.trim().is_empty()
                    && !language.contains(['"', '\\'])
                    && !language.chars().any(char::is_control),
                "invalid GitHub language qualifier"
            );
            terms.push(format!("language:\"{language}\""));
        }
        match (self.min_size_kb, self.max_size_kb) {
            (Some(min), Some(max)) => {
                ensure!(min <= max, "--min-size-kb must be at most --max-size-kb");
                terms.push(format!("size:{min}..{max}"));
            }
            (Some(min), None) => terms.push(format!("size:>={min}")),
            (None, Some(max)) => terms.push(format!("size:<={max}")),
            (None, None) => {}
        }
        if let Some(date) = &self.pushed_after {
            terms.push(format!("pushed:>{date}"));
        }
        terms.push(match stars_above {
            Some(stars) => format!("stars:>{stars}"),
            None => "stars:>=0".to_owned(),
        });
        let query = terms.join(" ");
        let per_page = self.limit.min(100);
        let github = Github::new(false)?;
        let mut remaining = usize::from(self.limit);
        for page in 1..=self.limit.div_ceil(per_page) {
            let response: Response = github.get(
                &["search", "repositories"],
                &[
                    ("q", &query),
                    ("per_page", &per_page.to_string()),
                    ("page", &page.to_string()),
                ],
            )?;
            if response.incomplete_results {
                eprintln!("warning: GitHub returned incomplete search results for page {page}");
            }
            let count = response.items.len();
            for item in response.items.into_iter().take(remaining) {
                if !visit(item, &github)? {
                    return Ok(());
                }
            }
            remaining = remaining.saturating_sub(count);
            if remaining == 0 || count < usize::from(per_page) {
                break;
            }
        }
        Ok(())
    }
}

pub(crate) fn date(input: &str) -> Result<String, String> {
    let invalid = || "expected a valid YYYY-MM-DD date".to_owned();
    let fields: Vec<_> = input.split('-').collect();
    if fields.len() != 3
        || fields[0].len() != 4
        || fields[1].len() != 2
        || fields[2].len() != 2
        || !fields
            .iter()
            .all(|field| field.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return Err(invalid());
    }
    let year: u16 = fields[0].parse().map_err(|_| invalid())?;
    let month: u8 = fields[1].parse().map_err(|_| invalid())?;
    let day: u8 = fields[2].parse().map_err(|_| invalid())?;
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        _ => 0,
    };
    if year == 0 || day == 0 || day > days {
        return Err(invalid());
    }
    Ok(input.to_owned())
}
