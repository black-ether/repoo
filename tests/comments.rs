use std::{
    fs,
    io::Write,
    process::{Command, Stdio},
};

#[test]
fn resolves_manifest_aliases_and_reports_missing_references() {
    let repo = tempfile::tempdir().unwrap();
    for (path, code) in [
        (
            "tsconfig.json",
            r#"{"compilerOptions":{"baseUrl":".","paths":{"@app/*":["src/*"]}}}"#,
        ),
        (
            "src/start.ts",
            "import { x } from '@app/helper';\nimport './missing';",
        ),
        ("src/helper.ts", "export const x = 1;"),
        ("Cargo.toml", "[workspace]\nmembers = [\"crates/a\"]"),
        (
            "crates/a/Cargo.toml",
            "[package]\nname=\"a\"\nversion=\"0.1.0\"\n[lib]\npath=\"code/root.rs\"",
        ),
        ("crates/a/code/root.rs", "mod helper;"),
        ("crates/a/code/helper.rs", "use crate::other::thing;"),
        ("crates/a/code/other.rs", "pub fn thing() {}"),
    ] {
        let file = repo.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, code).unwrap();
    }
    let result = Command::new(env!("CARGO_BIN_EXE_repoo"))
        .args(["analyze", "--format", "json"])
        .arg(repo.path())
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let rows: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        rows[0]["references"]["src/start.ts"],
        serde_json::json!(["src/helper.ts"])
    );
    assert_eq!(
        rows[0]["references"]["crates/a/code/helper.rs"],
        serde_json::json!(["crates/a/code/other.rs"])
    );
    assert_eq!(
        rows[0]["importers"]["src/helper.ts"],
        serde_json::json!(["src/start.ts"])
    );
    assert_eq!(rows[0]["reference_coverage"]["status"], "partial");
    assert!(
        rows[0]["reference_coverage"]["issues"]
            .as_array()
            .unwrap()
            .iter()
            .any(|issue| issue["message"].as_str().unwrap().contains("./missing"))
    );
}

#[test]
fn resolves_shell_headers_types_packages_and_ruby_files() {
    let repo = tempfile::tempdir().unwrap();
    for (path, code) in [
        (
            "scripts/start.ps1",
            ". \"$PSScriptRoot/helpers.ps1\"\nImport-Module '.\\Tools.psm1'",
        ),
        ("scripts/helpers.ps1", "# helper"),
        ("scripts/Tools.psm1", "# module"),
        ("scripts/start.sh", "source './common.sh'\n. \"$DYNAMIC\""),
        ("scripts/common.sh", "# helper"),
        ("src/main.c", "#include \"common.h\"\n#include <stdio.h>"),
        ("include/common.h", "void app(void);"),
        ("src/main.cpp", "#include \"view.hpp\""),
        ("src/view.hpp", "struct View {};"),
        ("App.cs", "using App.Tools;\nclass App { Helper helper; }"),
        (
            "OtherName.cs",
            "namespace App.Tools;\npublic class Helper {}",
        ),
        ("Unused.cs", "namespace App.Tools;\npublic class Unused {}"),
        ("Main.java", "import example.Helper;\nclass Main {}"),
        ("nested/Helper.java", "package example;\nclass Helper {}"),
        ("go.mod", "module example.org/app\n"),
        (
            "main.go",
            "package main\nimport (\n \"fmt\"\n x \"example.org/app/util\"\n)",
        ),
        ("util/a.go", "package util"),
        ("util/b.go", "package util"),
        ("util/a_test.go", "package util"),
        ("app.rb", "require_relative 'lib/util'"),
        ("lib/util.rb", "module Util\nend"),
    ] {
        let file = repo.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, code).unwrap();
    }
    let result = Command::new(env!("CARGO_BIN_EXE_repoo"))
        .args(["analyze", "--format", "json", "--select", "references"])
        .arg(repo.path())
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let rows: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        rows[0]["references"],
        serde_json::json!({
            "scripts/start.ps1":["scripts/Tools.psm1","scripts/helpers.ps1"],
            "scripts/start.sh":["scripts/common.sh"], "src/main.c":["include/common.h"],
            "src/main.cpp":["src/view.hpp"], "App.cs":["OtherName.cs"],
            "Main.java":["nested/Helper.java"], "main.go":["util/a.go","util/b.go"],
            "app.rb":["lib/util.rb"]
        })
    );
}

#[test]
fn resolves_local_file_references() {
    let repo = tempfile::tempdir().unwrap();
    for (name, content) in [
        ("lib.rs", "pub mod parser;\nmod missing;"),
        ("parser.rs", ""),
        (
            "cli.ts",
            "import { run } from './run';\nimport './run';\nimport 'external';",
        ),
        ("run.ts", ""),
        ("cli.py", "from . import helper"),
        ("helper.py", ""),
    ] {
        fs::write(repo.path().join(name), content).unwrap();
    }
    let result = Command::new(env!("CARGO_BIN_EXE_repoo"))
        .args(["analyze", "--format", "json", "--select", "references"])
        .arg(repo.path())
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let rows: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        rows[0]["references"],
        serde_json::json!({
            "lib.rs":["parser.rs"],"cli.ts":["run.ts"],"cli.py":["helper.py"]
        })
    );
}

#[test]
fn inventories_files_once_and_preserves_counting_exclusions() {
    let repo = tempfile::tempdir().unwrap();
    for file in [
        "src/main.rs",
        "src/router.test.ts",
        "contest.py",
        "tests/fixtures/input.json",
        ".github/workflows/test.yml",
        "vendor/lib/tests/foo_test.go",
        "docs/logo.png",
        "README.md",
        "unknown.xyz",
        ".gitignore",
        "ignored/cache.rs",
    ] {
        let path = repo.path().join(file);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            path,
            if file == ".gitignore" {
                "ignored/\n"
            } else {
                "content\n"
            },
        )
        .unwrap();
    }
    let result = Command::new(env!("CARGO_BIN_EXE_repoo"))
        .args(["analyze", "--format", "json"])
        .arg(repo.path())
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let rows: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let groups = rows[0]["groups"].as_array().unwrap();
    let expected = [
        ("source", 2),
        ("tests", 1),
        ("config_build", 2),
        ("docs_assets", 2),
        ("data_vendored_generated", 2),
    ];
    for (row, (name, files)) in groups.iter().zip(expected) {
        assert_eq!(row["group"], name);
        assert_eq!(row["files"], files);
    }
    assert_eq!(groups[0]["key_files"][0]["path"], "src/main.rs");
    assert_eq!(groups[4]["counted_files"], 1);
    assert_eq!(rows[0]["unclassified_files"], 1);
}

#[test]
fn analyzes_ranked_paths_and_language_share() {
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir(repo.path().join("src")).unwrap();
    fs::create_dir(repo.path().join("tests")).unwrap();
    fs::write(
        repo.path().join("src/main.rs"),
        "fn main() {}\nfn second() {}\n",
    )
    .unwrap();
    fs::write(
        repo.path().join("tests/check.rs"),
        "// note\nfn check() {}\n",
    )
    .unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_repoo"))
        .args(["analyze", "--format", "ndjson"])
        .arg(repo.path())
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let row: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(row["largest_directories"][0]["path"], "src");
    assert_eq!(row["largest_directories"][0]["code"], 2);
    assert_eq!(row["largest_files"][0]["path"], "src/main.rs");
    assert_eq!(row["largest_files"][1]["comments"], 1);
    assert_eq!(row["languages"]["Rust"]["share_pct"], 100.0);
}

#[test]
fn counts_and_skips_known_fixture() {
    let repo = tempfile::tempdir().unwrap();
    fs::create_dir(repo.path().join("vendor")).unwrap();
    fs::write(repo.path().join(".gitignore"), "ignored.rs\n").unwrap();
    fs::write(repo.path().join("lib.rs"), "// comment\n\nfn demo() {}\n").unwrap();
    fs::write(repo.path().join("ignored.rs"), "fn ignored() {}\n").unwrap();
    fs::write(repo.path().join("vendor/excluded.rs"), "fn excluded() {}\n").unwrap();
    fs::write(repo.path().join("binary.rs"), b"\0").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_repoo"))
        .args(["comments", "--format", "ndjson"])
        .arg(repo.path())
        .output()
        .unwrap();
    assert!(result.status.success(), "{:?}", result);
    let row: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(row["schema"], 1);
    assert_eq!(row["error"], serde_json::Value::Null);
    assert_eq!(row["total_code_lines"], 1);
    assert_eq!(row["comment_pct"], 50.0);
    assert_eq!(
        row["languages"]["Rust"],
        serde_json::json!({"code":1,"comments":1,"blanks":1,"files":1})
    );
}

#[test]
fn merges_stdin_records_and_keeps_batch_errors() {
    let repo = tempfile::tempdir().unwrap();
    fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_repoo"))
        .args([
            "comments",
            "--stdin",
            "--format",
            "json",
            "--concurrency",
            "1",
            "--where",
            "code_lines<5",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let prefilter = serde_json::json!({
        "schema": 1, "repo": "example/fixture", "phase": "prefilter",
        "error": {"code": "github_failed", "message": "no tree"},
        "path": repo.path(),
    });
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{prefilter}").unwrap();
    writeln!(stdin, "{{broken").unwrap();
    drop(stdin);
    let result = child.wait_with_output().unwrap();
    assert_eq!(result.status.code(), Some(2));
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(rows.len(), 2);
    let merged = rows
        .iter()
        .find(|row| row["repo"] == "example/fixture")
        .unwrap();
    assert_eq!(merged["prefilter"], prefilter);
    assert_eq!(merged["total_code_lines"], 1);
    assert!(merged["error"].is_null());
    assert!(
        rows.iter()
            .any(|row| row["error"]["code"] == "invalid_input")
    );
}

#[test]
fn selects_fields_after_filtering_and_retains_record_identity() {
    let repo = tempfile::tempdir().unwrap();
    fs::write(repo.path().join("main.rs"), "fn main() {}\n").unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_repoo"))
        .args([
            "comments",
            "--select",
            // Exercise the streaming JSON projection independently of the TOON CLI.
            "files,comment_pct",
            "--format",
            "ndjson",
            "--where",
            "code_lines==1",
        ])
        .arg(repo.path())
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    let row: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        row,
        serde_json::json!({
            "schema": 1, "phase": "comments", "error": null, "path": repo.path(),
            "files": 1, "comment_pct": 0.0,
        })
    );
}
