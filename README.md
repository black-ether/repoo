# repoo

**Find your next repository. Know where to start reading.**

[![Rust](https://img.shields.io/badge/Rust-1.88%2B-orange?logo=rust)](https://www.rust-lang.org/)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

One CLI for exploring GitHub and making sense of a local codebase. Find small
projects worth studying, then map the files that matter: entry points,
heavily referenced modules, tests, and configuration.

~~~sh
# Get oriented in any local checkout.
repoo analyze .

# Find five small Rust projects with more than 10 stars.
repoo discover 'lang=rust,size=0-300KB,stars>10,feed=5'

# Export a focused repository map for a script or coding agent.
repoo analyze . --no-tree --format json --select groups,references,importers
~~~

## Why repoo?

- **Choose what to read.** Search GitHub by language, size, stars, and last push date. Optionally filter by comment percentage.
- **Find your bearings.** See language totals, entry points, import hubs, large source files, and useful config and test paths.
- **Follow local connections.** Inspect detected imports and reverse references without setting up a language server.
- **Keep the output useful.** Read a compact terminal table, stream NDJSON through a pipeline, or export JSON for your own tools.

Built in Rust, with Tokei for line counting and a shared Rayon worker pool.

For example, analyzing repoo itself reveals where to start reading
(excerpt from the terminal output):

```text
ENTRY & HUBS
  src/main.rs (entry) ──▶ comments, discover, github, inventory, output, prefilter, references, search, tree
  hubs (by incoming refs): src/comments.rs (3) · src/discover.rs (2) · src/github.rs (2) · src/output.rs (2)
  coverage: partial (lexical scan, 0 reported issues)
```

## Install

From a checkout, with **Rust 1.88+**:

~~~sh
git clone https://github.com/black-ether/repoo.git
cd repoo
cargo install --path . --locked
repoo --help
~~~

Local analysis needs no GitHub credentials. For GitHub requests, repoo checks
GITHUB_TOKEN, then tries gh auth token, then falls back to the unauthenticated
quota with a warning. Git is required when a command clones a repository.

### Binary builds

The [build workflow](.github/workflows/build.yml) packages Windows x64 and ARM64,
Linux x64 and ARM64, and macOS Intel and Apple Silicon. Open **Actions →
Build binaries → a successful run → Artifacts** on GitHub to download a package.
Extract it and put repoo (or repoo.exe) on your PATH. Packages include the
executable, README, license, copyright notice, and a SHA-256 checksum.

Builds run on pushes, pull requests, version tags, or a manual workflow dispatch.
Artifacts become available after the workflow succeeds. You can always build
locally with cargo build --release --locked.

## Explore a local codebase

~~~sh
# Languages, entry points, hubs, file groups, and a directory tree.
repoo analyze .

# Compare multiple checkouts in one invocation.
repoo analyze ./frontend ./backend

# Focus on the largest files and language breakdown.
repoo analyze . --no-tree --format json --select languages,largest_files

# See local dependencies and the files that import them.
repoo analyze . --no-tree --format json --select references,importers,reference_coverage

# Keep successful results above 1,000 code lines and 10% comments.
repoo analyze ./frontend ./backend --where 'code_lines>1000 && comment_pct>10'
~~~

The terminal view highlights entry points, import hubs, selected reference edges,
four large source/test files, and key paths. Structured output includes the full
detected reference graph and rankings of up to five files and directories.
The --select option keeps requested top-level fields plus identifiers and errors.

| Option | Effect |
| --- | --- |
| --tree-depth 0 | Show root entries only. Default 1 also shows their children. |
| --no-tree-sizes | Show the tree without recursively calculating directory sizes. |
| --no-tree | Skip the tree and total-size scan; useful for large checkouts. |
| --concurrency 4 | Cap the shared worker pool at four threads. |

Line counts follow ignore rules and normally exclude vendor, generated,
minified, and binary-looking files. Opt in with --include-vendor,
--include-generated, --include-minified, --include-binary, or --include-ignored.
Other inclusion flags still respect ignore rules.

Tree sizes include ignored build and dependency files, exclude Git metadata,
and never follow symlinks. Large target/ or node_modules/ directories can make
the size scan slower. Sizes sum file lengths in decimal units, rather than
allocated disk space. Read errors produce partial sizes and a notice.

## Discover something worth reading

~~~sh
# Small projects with recent activity.
repoo discover 'lang=rust,size=0-300KB,stars>10,after=20260101,feed=5'

# Add file counts and byte-based code/prose shares without cloning.
repoo discover 'lang=python,size=0-500KB,stats=true,feed=5'

# Find comment-rich projects; shallow-clones candidates as needed.
repoo discover 'keyword=parser,lang=rust,comment>10,limit=50,feed=5'

# Inspect a known repository; tree stats are on by default here.
repoo discover 'repo=chrislgarry/Apollo-11'

# Export discovery results.
repoo discover 'lang=rust,size=0-300KB,feed=5,format=json'
~~~

Quote the **whole query**, especially in PowerShell.

| Query term | Meaning |
| --- | --- |
| keyword=parser | GitHub repository search keyword. |
| lang=rust | Primary repository language. |
| size=0-300KB | Inclusive range using GitHub's reported repository size. |
| stars>10 | Strictly more than 10 stars. |
| after=20260101 | Last push after the date; 2026-01-01 also works. |
| comment>10 | More than 10% comment lines: 100 × comments / (code + comments). Requires local counting. |
| limit=50 | Inspect at most 50 candidates; default 20, maximum 1,000. |
| feed=5 | Stop after five successful matches. Errors remain visible and do not count toward this cap. |
| stats=true | Add Git tree file counts and byte-based composition. |
| format=json | Choose table (default), json, ndjson, or toon. |
| repo=owner/name | Inspect one known repository instead of searching; stats=false skips its tree lookup. |

Without comment filtering, discovery uses GitHub metadata. Tree stats fetch
paths and blob sizes without downloading file contents. Comment filtering
shallow-clones into .reporeport-cache/owner/repo/commit-sha. Cached checkouts
stay pinned to their saved commit and reuse counts for the same flags; treat
them as immutable. Cloning uses Git's credentials with prompts disabled.

## Build a pipeline

Analyze one local checkout per line in repos.txt:

~~~sh
repoo analyze --stdin --no-tree --format ndjson < repos.txt
~~~

In PowerShell:

~~~powershell
Get-Content repos.txt | repoo analyze --stdin --no-tree --format ndjson
~~~

Or discover remote repositories and analyze their checkouts:

~~~sh
repoo discover 'lang=rust,size=0-300KB,feed=3,format=ndjson' | repoo analyze --stdin --clone-missing --no-tree --format ndjson
~~~

Use **NDJSON between commands**: each line is one record, emitted as work
completes. JSON produces one array. TOON produces one document after the batch
and requires the official toon CLI on PATH (npm install -g @toon-format/cli).
Logs go to stderr.

| Exit code | Meaning |
| --- | --- |
| 0 | Success. |
| 1 | Fatal error. |
| 2 | One or more repository failures; inspect result errors. |

The --where option (alias --filter) accepts top-level numeric comparisons joined
by &&: <, <=, >, >=, ==, and !=. Failed records remain visible.
The code_lines field aliases total_code_lines.

The hidden comments and prefilter commands remain available for existing
scripts. See the [schema reference](docs/schema.md) for field definitions,
import resolution, and error shapes. Print the JSON Schema with repoo schema.

## Know the boundaries

- **References are hints.** Lexical rules detect local imports in Rust, JavaScript/TypeScript, Python, PowerShell, Bash, C/C++, C#, Java, Go, and Ruby. Dynamic loading, build conditions, and some manifest features leave gaps. Coverage is partial; see the [resolution details](docs/schema.md#local-analysis).
- **Bytes and lines measure different things.** Tree composition estimates use paths, extensions, and blob sizes. Local counts use files Tokei recognizes, including supported prose and configuration languages.
- **GitHub has limits.** Search caps results at 1,000 and can return incomplete pages. Large recursive trees can be truncated; the truncated field marks partial tree metrics.
- **No complexity score yet.** Schema v1 reserves complexity for a future extension.

## Develop

~~~sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
~~~

Tests use local fixtures and recorded tree responses. Live GitHub and clone
checks need network access. Benchmark your checkout with
[Hyperfine](https://github.com/sharkdp/hyperfine):

~~~sh
hyperfine --warmup 5 'target/release/repoo analyze ./checkout --no-tree --format ndjson' 'target/release/repoo analyze ./checkout --no-tree --format ndjson --concurrency 1'
~~~

On Windows, use target/release/repoo.exe. Representative large-repository
benchmarks are still needed; performance depends on checkout contents and
whether recursive tree sizes are enabled.

## License

Copyright 2026 **Black-Ether (JoJo)**. Licensed under the
[Apache License, Version 2.0](LICENSE).
