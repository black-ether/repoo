# Record schema v1

Each NDJSON line and each item in a JSON or TOON array is a record. The default
format is a human-readable table. TOON emits one document when the batch
finishes. NDJSON emits records as repositories complete. Search records follow GitHub
ranking and have their own shape. `repoo schema` prints the
[JSON Schema](schema.json) for individual records.

`--select` projects top-level fields from analyze and compatibility commands.
It preserves `schema`, `phase`, `error`, and any `repo` or `path` identifiers.
Selected fields that are absent are omitted. The JSON Schema describes full
records. Select fields at the end of a pipeline to retain upstream metrics.

## Common phase fields

| Field | Meaning |
| --- | --- |
| `schema` | Integer schema version, currently 1. |
| `repo` | GitHub `owner/repo` slug. Required for prefilter. Present in comments when carrying a prefilter record. |
| `path` | Local directory in comments output. On failure it identifies the requested input. |
| `phase` | `prefilter` or `comments`. |
| `error` | Null on success. Otherwise an object with machine-readable `code` and human-readable `message`. Metrics may be absent on failure. |
| `complexity` | Reserved object for future versions. |

Error codes are `invalid_input` for malformed stdin records or GitHub
slugs, `github_failed` for a failed repository API request, and
`count_failed` for local path, clone, or read errors. Errors remain visible
when a filter is active. Malformed stdin records carry a line label as
their identifier.

## Prefilter

| Field | Meaning |
| --- | --- |
| `size_kb` | Repository size in KB as reported by GitHub. |
| `description` | GitHub repository description or null. |
| `top_language` | First entry in `languages` as an object with `language` and `pct`, or null when no eligible code language was found. |
| `files`, `total_bytes` | Number and summed size in bytes of returned blobs, including skipped blobs. Tree entries and submodules are excluded. Missing blob sizes contribute zero. |
| `classes` | Objects `code`, `prose`, `data_config`, and `other_binary`, each with integer `bytes` and `files` across all returned blobs. |
| `skipped` | Integer `bytes` and `files` excluded by skip rules. |
| `prose_pct`, `code_pct` | 0–100, using included class bytes divided by all included blob bytes. Both are zero when no included bytes exist. |
| `languages` | Up to 10 entries sorted by descending included code bytes, then by language name. Each has `language`, `bytes`, and `pct` (0–100 share of all included code bytes). |
| `truncated` | True if GitHub truncated the tree. All metrics describe the returned portion. |

## Local analysis

`repoo analyze` uses the existing `comments` phase in structured output.
Each language adds `share_pct`, its percentage of all counted code lines.
`largest_directories` and `largest_files` each contain up to five entries,
ordered by descending code lines, then by path. Each entry has a
repository-relative `path`, `code`, `comments`, `blanks`, `files`,
and `share_pct`. Directory totals cover top-level directories recursively.
Root-level files contribute to the repo total and file ranking.

The default table summarizes entry points, hubs, selected reference edges,
the four largest Source or Tests files, and config, documentation, and test
paths. `largest_code_files` ranks up to five Source or Tests files.
`fixture_directories` maps visible fixture directory paths to inventoried
file counts. Group `key_files` entries also include their counted `code` lines.

`tree` includes a root `name`, `entries`, read-error `issues`, and
a boolean `sizes` indicating whether sizes were requested.
Each entry has a relative `path`, `kind` (file, directory, or symlink),
and `bytes`. The tree displays root entries and their immediate children.
Directory sizes sum file lengths recursively, including ignored files.
Git metadata directories are excluded and symlinks are shown with zero
bytes without following them. Read errors produce partial sizes and are
listed in `issues`. Tree file totals can exceed the counted inventory.
Size labels use decimal KB, MB, and GB. Calculating sizes traverses ignored
build and dependency directories, which can add substantial scan time.
The table's top-line total sums `bytes` for root-level tree entries,
including the recursive directory totals exactly once. It measures file
lengths, including ignored files, with rounded decimal units for display.
The summary file count continues to refer to the analysis inventory.
With `--no-tree-sizes`, entry `bytes` values are null. With
`--no-tree`, the entire `tree` field is omitted.

`groups` lists Source, Tests, Config / Build, Docs / Assets, and Data /
Vendored / Generated in that order. Each group has a snake_case `group`
identifier, `files` inventoried, `counted_files`, counted `code`,
`comments`, `blanks`, and up to three `key_files` with a relative `path`
and a selection `reason`. `unclassified_files` counts unmatched files.
Inventory follows the walker's ignore rules. Visible vendored and generated
paths can appear even when their contents are excluded from line counts.
Rules classify whole files, including source modules containing inline tests.

`references` maps a source file to its sorted, unique local target paths.
The table highlights entry-point edges and up to three additional edges.
The scanner matches conventional Rust
`mod name;` declarations and the first module in `use crate::name::…`,
static relative JavaScript/TypeScript imports
and re-exports, and Python relative `from` imports against counted files.
Additional rules cover the following local references:

| Language | Recognized reference |
| --- | --- |
| PowerShell | Dot-sourcing, `Import-Module`, and `using module` with literal paths, including the `$PSScriptRoot` prefix. |
| Bash | Literal `source` and dot commands. Relative paths are interpreted from the script directory. |
| C and C++ | Quoted includes searched beside the source file, under the repository's `include/`, then at the repository root. |
| C# | Namespace imports with a matching locally declared type name mentioned in the source, and explicit static or alias imports of a local type. |
| Java | Explicit imports matched against the declared package and filename of local Java sources. |
| Go | Single and grouped module imports matched using the nearest `go.mod`. Each arrow points to a counted non-test Go file in the imported package. Platform and build tags are ignored. |
| Ruby | Literal `require_relative` paths with or without the `.rb` suffix. |

These are syntactic hints. Multiline literals or comments can resemble imports,
and dynamic loading and external packages can leave gaps. Manifest-aware
resolution supports the aliases and target paths described below.

`importers` reverses `references`: each target maps to its sorted, unique
source files. The table lists the four targets with the most importers.
`reference_coverage` has `status`, `method`, `issues` (objects with
`path` and `message`), and `limitations`. Lexical scans report `partial`
even when no issue was detected. Issues currently cover unresolved extracted
local path imports and unreadable or unsupported project configuration.
External imports and unresolved symbol or package imports can still be omitted.

Resolution reads the nearest strict-JSON `tsconfig.json` and handles
`compilerOptions.paths` exact and wildcard mappings relative to `baseUrl`.
Config inheritance and JSON comments require fallback and are reported.
Cargo manifests bound `crate::` resolution to their package and identify
custom library, binary, test, example, and bench target paths. Cargo dependency
renames, feature evaluation, and full workspace dependency resolution require
further analysis.

| Field | Meaning |
| --- | --- |
| `code`, `total_code_lines` | The same integer count of code lines. |
| `comments` | Integer count of comment lines. |
| `blanks` | Integer count of blank lines. |
| `files` | Integer count of recognized, counted files. |
| `comment_pct` | `100 × comments / (code + comments)`, or zero when the denominator is zero. |
| `languages` | Object keyed by Tokei language name. Each value has integer `code`, `comments`, `blanks`, and `files`. Embedded language lines roll into their containing file’s language. |
| `prefilter` | Original prefilter object when provided on stdin, including its own metrics and error state. |

Tokei’s parsing and classification determine the line categories, including
how lines containing both code and comments are counted. Files recognized
as prose or configuration by Tokei can contribute to these totals.

## Discovery results

Without a comment filter, `discover` emits the fields below. With a comment
filter it emits comments records with `repo`, `size_kb`, `stars`,
`pushed_at`, and `language` added.

With `stats=true`, a successful tree lookup adds `tree_files`, `code_pct`,
`prose_pct`, and `truncated` using the prefilter definitions. `tree_files`
counts returned Git blobs. In comment-filtered records, `files` counts the
local files analyzed by Tokei. A failed tree lookup sets `error`.

| Field | Meaning |
| --- | --- |
| `full_name` | GitHub `owner/repo` slug. |
| `size_kb` | GitHub repository size in KB, as returned by GitHub. |
| `stars` | Number of GitHub stargazers. |
| `pushed_at` | Last push timestamp or null. |
| `language` | Primary language reported by GitHub or null. |
