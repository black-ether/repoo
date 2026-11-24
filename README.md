# repoo

Find GitHub repositories and analyze local checkouts with code counts,
file groups, entry points, and local file references.

## Install

Requires Rust 1.88 or newer and Cargo.

```sh
cargo install --path . --locked
```

Git is needed for `--clone-missing`. For GitHub API requests, repoo
looks for `GITHUB_TOKEN`, then tries `gh auth token`, then uses GitHub’s
unauthenticated quota and emits a warning. Git cloning uses Git’s own
credential configuration, with prompts disabled.

## Workflow

```sh
repoo discover 'keyword=pixle,lang=rust,size=100-300KB,comment>10,stars>10,after=20230429,feed=5,format=json'
repoo discover 'lang=rust,size=0-300KB,stats=true,feed=5'
repoo discover 'repo=chrislgarry/Apollo-11'
repoo analyze ./ripgrep ./log
repoo analyze . --format json --select languages,largest_files
repoo analyze . --format json --select groups,unclassified_files
repoo analyze . --format json --select references
repoo analyze . --format json --select references,importers,reference_coverage
```

Quote the whole discovery query in PowerShell. Size ranges are inclusive.
`stars>10` requires more than 10 stars and `after=20230429` filters by
push date. `comment>10` shallow-clones candidates as needed and keeps
repos with more than 10% comment lines. Omit that filter for GitHub
metadata only. `limit` caps GitHub candidates and defaults to 20.
`feed=5` stops after five successful matches. Error records still appear
and do not count toward that cap.
The default output format is table.

`analyze` summarizes local code, comments, blank lines, and counted files.
The compact table shows language totals, entry points, hubs, selected edges,
the four largest code files, and key config, documentation, and test paths.
Structured output includes all languages, five file and directory rankings,
file groups, and the full detected reference graph. Reference coverage is
reported as partial because detection uses language-specific lexical rules.
See the [schema reference](docs/schema.md) for supported import forms and
manifest-resolution limits. The original `comments` command remains available
for scripts.

Use `stats=true` to add the repository file count and byte-based code and
prose shares. It fetches the Git tree for each qualifying repository.
With `repo=owner/name`, discovery looks up that repository directly and
includes tree stats by default. Use `stats=false` to skip the tree request.
Both modes accept `comment>…` and `format=…`. The `prefilter` command
remains available for existing scripts and its full tree breakdown.

```powershell
repoo discover 'lang=rust,feed=5,stats=true'
```

The hidden compatibility command `prefilter` makes one repository metadata
request to find the default branch and one
recursive Git Trees request per repository, before retries. It requests
tree metadata and blob sizes without cloning or reading blob contents.
It processes eight repositories concurrently by default. Discover checks
candidates sequentially and stops when `feed` is satisfied or `limit` is reached.
Blocking reqwest keeps
HTTP scheduling in the bounded worker pool, and reuses one Rustls client.
`--verbose` reports rate-limit headers. Retries share a cooldown and wait
up to 60 seconds per delay.

Prefilter skips common vendor, generated, minified, and lockfile paths.
For `prefilter`, use repeatable `--skip '**/fixtures/**'` patterns to add exclusions, or
`--no-default-skip` to replace the defaults. Comments respects local ignore
rules and skips vendor, generated, minified, and binary-looking files by
default. Its `--include-*` flags let you opt in. Existing paths take
precedence over GitHub slugs. Cloned checkouts live under
`CACHE/owner/repo/commit-sha`. Cache hits stay pinned to the saved commit and reuse counts for the same flags. Treat cached checkouts as immutable.

See the [field reference](docs/schema.md) and run `repoo schema` to
get the machine-readable schema. The default is a human-readable table.
Use `--format ndjson` on each producer that feeds another repoo command,
or `--format json` for a single JSON array. `--format toon` writes one
document after the batch completes and requires the official `toon` CLI
on PATH (`npm install -g @toon-format/cli`).
Logs go to stderr. Exit codes are 0 for success, 1 for fatal
errors, and 2 when any repository failed. On `analyze` and the compatibility
commands, `--where` (alias `--filter`)
accepts numeric comparisons joined by `&&`. Available operators are
`<`, `<=`, `>`, `>=`, `==`, and `!=`. Use top-level numeric phase
metrics in the expression, with `code_lines` as an alias for
`total_code_lines`. Merged outputs also support prefilter-only metrics.

## Performance

The local walker applies ignore and exclusion rules before reading.
Tokei parses the selected files as a library. Both repository work and
per-file parsing use one Rayon pool, so their parallelism shares the same
thread budget. Compare the default against one worker on a warm checkout
near 50,000 lines using Hyperfine:

```sh
cargo build --release
hyperfine --warmup 5 'target/release/repoo analyze ./checkout --format ndjson' 'target/release/repoo analyze ./checkout --format ndjson --concurrency 1'
hyperfine --warmup 3 'target/release/repoo analyze --stdin --format ndjson < repos.txt'
```

Use a `repos.txt` with one local checkout per line for the batch test and
add `.exe` to the executable path on Windows. Local measurements appear
below. The 50,000-line and 100-repository performance targets still need
representative benchmark checkouts.

On Windows with Rust 1.100.0, ten warm release runs of
`repoo analyze . --format json` on this project's 16 counted files
(3,462 code lines) measured 24.48 ms median, 23.87 ms minimum, and
39.99 ms maximum on November 25, 2026. PowerShell's stopwatch included
process startup and output consumption. These measurements cover this
small checkout.

## Development checks

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

Tests use local fixtures and recorded tree-response examples. Live GitHub
and clone verification requires network access. JSON, NDJSON, and table
output run in the Rust binary. TOON output invokes the installed encoder.

## Limitations

- Prefilter estimates from extensions, paths, and bytes. Comments provides
  line-exact counts for the files Tokei recognizes.
- GitHub’s Languages API omits prose languages, so prefilter uses Git Trees.
- GitHub can truncate very large recursive trees. Prefilter reports the
  partial metrics and sets `truncated`.
- GitHub caps a search at 1,000 results and can return an incomplete page.
- Binary detection in comments checks for NUL bytes after BOM-aware
  decoding. Files without a recognized language are outside the line counts.
- Schema v1 reserves `complexity` for a future extension.

GitHub references
[Git Trees](https://docs.github.com/en/rest/git/trees),
[Search](https://docs.github.com/en/rest/search/search#search-repositories),
[Rate limits](https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api).

