use std::{cmp::Reverse, collections::BTreeSet, path::PathBuf, sync::LazyLock};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde_json::{Value, json};

use crate::comments::{GENERATED, LOCKS, Lines, MINIFIED, VENDOR, generated_file};

const GROUPS: [&str; 5] = [
    "source",
    "tests",
    "config_build",
    "docs_assets",
    "data_vendored_generated",
];

// First matching rule wins. Priority controls representative-file selection.
const RULES: &[(&str, usize, &str, u8)] = &[
    (
        "**/{fixtures,testdata,__fixtures__}/**",
        4,
        "fixture data",
        1,
    ),
    ("**/{.github/workflows,.circleci}/**", 2, "CI workflow", 2),
    (
        "**/{cargo.toml,package.json,pyproject.toml,go.mod,pom.xml,build.gradle,build.gradle.kts,*.csproj}",
        2,
        "manifest",
        0,
    ),
    (
        "**/{makefile,gnumakefile,cmakelists.txt,dockerfile,dockerfile.*,build.rs,meson.build,build.zig}",
        2,
        "build definition",
        1,
    ),
    ("**/{tests,test,spec,__tests__}/**", 1, "test suite", 1),
    (
        "**/{*_test.go,test_*.py,*_test.py,*.test.*,*.spec.*,*_spec.rb,*Test.java,*Test.kt}",
        1,
        "test file",
        2,
    ),
    ("**/{readme,readme.*}", 3, "README", 0),
    (
        "**/*.{png,jpg,jpeg,gif,svg,webp,ico,woff,woff2,ttf,pdf,mp3,mp4}",
        3,
        "asset",
        3,
    ),
    ("**/{docs,doc}/**", 3, "guide", 1),
    ("**/*.{md,mdx,rst,adoc,txt}", 3, "documentation", 2),
    ("**/{assets,images,fonts}/**", 3, "asset", 3),
    ("**/{data,datasets}/**", 4, "dataset", 1),
    (
        "**/*.{csv,tsv,jsonl,ndjson,parquet,sqlite,db}",
        4,
        "dataset",
        1,
    ),
    ("**/{config,configs}/**", 2, "configuration", 3),
    (
        "**/{.gitignore,.gitattributes,.editorconfig,tsconfig*.json,*.config.*,*.yaml,*.yml,*.toml,*.ini,*.cfg}",
        2,
        "configuration",
        3,
    ),
    ("**/*.json", 4, "data file", 3),
    (
        "**/{src/main.rs,src/lib.rs,main.go,__main__.py,main.py,main.c,main.cpp,program.cs}",
        0,
        "entry point",
        0,
    ),
];

static RULE_SET: LazyLock<GlobSet> = LazyLock::new(|| {
    let mut builder = GlobSetBuilder::new();
    for (pattern, ..) in RULES {
        builder.add(
            GlobBuilder::new(pattern)
                .literal_separator(true)
                .case_insensitive(true)
                .build()
                .expect("valid inventory glob"),
        );
    }
    builder.build().expect("valid inventory rules")
});

fn classify(path: &str, counted: bool) -> Option<(usize, &'static str, u8)> {
    let lower = path.to_ascii_lowercase();
    let (parent, name) = lower.rsplit_once('/').unwrap_or(("", lower.as_str()));
    if parent.split('/').any(|part| VENDOR.contains(&part)) {
        return Some((4, "vendored dependency", 2));
    }
    if LOCKS.contains(&name) || name.ends_with(".lock") {
        return Some((4, "dependency lockfile", 2));
    }
    if generated_file(name)
        || parent.split('/').any(|part| GENERATED.contains(&part))
        || MINIFIED.iter().any(|suffix| name.ends_with(suffix))
    {
        return Some((4, "generated file", 2));
    }
    if let Some(&index) = RULE_SET.matches(path).first() {
        let (_, group, reason, priority) = RULES[index];
        return Some((group, reason, priority));
    }
    counted.then_some((0, "module", 2))
}

pub fn is_code(path: &str) -> bool {
    classify(path, true).is_some_and(|(group, _, _)| group <= 1)
}

#[derive(Default)]
struct Group {
    files: usize,
    counted_files: usize,
    code: usize,
    comments: usize,
    blanks: usize,
    candidates: Vec<(u8, usize, Reverse<usize>, String, &'static str)>,
}

pub fn summarize(scanned: &[(PathBuf, Lines)]) -> (Value, usize) {
    let mut groups: [Group; 5] = Default::default();
    let mut unknown = 0;
    for (path, lines) in scanned {
        let path = path
            .components()
            .map(|p| p.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let Some((index, mut reason, rank)) = classify(&path, lines.files > 0) else {
            unknown += 1;
            continue;
        };
        if reason == "manifest" && !path.contains('/') {
            reason = "root manifest";
        }
        let group = &mut groups[index];
        group.files += 1;
        group.counted_files += lines.files;
        group.code += lines.code;
        group.comments += lines.comments;
        group.blanks += lines.blanks;
        let depth = if rank == 0 {
            path.matches('/').count()
        } else {
            0
        };
        group
            .candidates
            .push((rank, depth, Reverse(lines.code), path, reason));
    }
    let rows: Vec<_> = groups.into_iter().enumerate().map(|(index, mut group)| {
        group.candidates.sort_unstable();
        let mut selected = Vec::new();
        let mut reasons = BTreeSet::new();
        let mut parents = BTreeSet::new();
        for pass in 0..3 {
            for (_, _, _, path, reason) in &group.candidates {
                if selected.len() == 3 { break; }
                let parent = path.rsplit_once('/').map(|(p, _)| p).unwrap_or("");
                if selected.contains(&(path, reason)) || (pass == 0 && reasons.contains(reason)) || (pass == 1 && parents.contains(parent)) { continue; }
                selected.push((path, reason));
                reasons.insert(*reason);
                parents.insert(parent);
            }
        }
        let key_files: Vec<_> = selected.into_iter().map(|(path,reason)| {
            let code = group.candidates.iter().find(|(_,_,_,p,_)| p == path).map(|(_,_,Reverse(code),_,_)| *code).unwrap_or_default();
            json!({"path":path,"reason":reason,"code":code})
        }).collect();
        json!({"group": GROUPS[index], "files":group.files,"counted_files":group.counted_files,"code":group.code,"comments":group.comments,"blanks":group.blanks,"key_files":key_files})
    }).collect();
    (json!(rows), unknown)
}
