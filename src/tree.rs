use std::{collections::BTreeMap, io::Write, path::Path};

use anyhow::Result;
use ignore::WalkBuilder;
use serde_json::{Value, json};

/// Inventory the displayed levels and optionally sum recursive sizes.
pub fn scan(root: &Path, depth: usize, sizes: bool) -> Value {
    let mut entries = BTreeMap::<String, Value>::new();
    let mut issues = Vec::new();
    let mut walker = WalkBuilder::new(root);
    walker
        .max_depth((!sizes).then_some(depth))
        .standard_filters(false)
        .follow_links(false)
        .filter_entry(|entry| entry.depth() == 0 || entry.file_name() != ".git");
    for entry in walker.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                issues.push(error.to_string());
                continue;
            }
        };
        if entry.depth() == 0 {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        let components: Vec<_> = relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let key = components.join("/");
        let kind = entry.file_type();
        let directory = kind.is_some_and(|t| t.is_dir());
        let bytes = if sizes && kind.is_some_and(|t| t.is_file()) {
            match entry.metadata() {
                Ok(meta) => meta.len(),
                Err(error) => {
                    issues.push(error.to_string());
                    0
                }
            }
        } else {
            0
        };
        if components.len() <= depth {
            entries.insert(key.clone(),json!({
                "path":key,"kind":if directory {"directory"} else if kind.is_some_and(|t|t.is_symlink()) {"symlink"} else {"file"},
                "bytes":sizes.then_some(0)
            }));
        }
        if bytes == 0 {
            continue;
        }
        for depth in 1..=components.len().min(depth) {
            let ancestor = components[..depth].join("/");
            if let Some(row) = entries.get_mut(&ancestor) {
                row["bytes"] = json!(
                    row["bytes"]
                        .as_u64()
                        .unwrap_or_default()
                        .saturating_add(bytes)
                );
            }
        }
    }
    let name = root
        .canonicalize()
        .ok()
        .and_then(|path| path.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| root.display().to_string());
    json!({"name":name,"entries":entries.into_values().collect::<Vec<_>>(),"issues":issues,"sizes":sizes})
}

pub fn size(bytes: u64) -> String {
    let mut amount = bytes as f64;
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut unit = 0;
    while amount >= 1000.0 && unit < units.len() - 1 {
        amount /= 1000.0;
        unit += 1;
    }
    format!("{amount:.0} {}", units[unit])
}

pub fn render(out: &mut impl Write, tree: &Value) -> Result<()> {
    writeln!(
        out,
        "\n{}/",
        tree["name"].as_str().unwrap_or(".").escape_debug()
    )?;
    let mut children = BTreeMap::<&str, Vec<&Value>>::new();
    for row in tree["entries"].as_array().into_iter().flatten() {
        let path = row["path"].as_str().unwrap_or("");
        let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
        children.entry(parent).or_default().push(row);
    }
    render_children(out, &children, "", "")?;
    let issues = tree["issues"].as_array().map_or(0, Vec::len);
    if issues > 0 {
        writeln!(out, "  tree sizes partial ({issues} read errors)")?;
    }
    Ok(())
}

fn render_children(
    out: &mut impl Write,
    children: &BTreeMap<&str, Vec<&Value>>,
    parent: &str,
    indent: &str,
) -> Result<()> {
    if let Some(rows) = children.get(parent) {
        for (index, row) in rows.iter().enumerate() {
            let last = index + 1 == rows.len();
            let path = row["path"].as_str().unwrap_or("");
            let name = path.rsplit('/').next().unwrap_or("");
            let branch = if last { "╰── " } else { "├── " };
            write_row(out, &format!("{indent}{branch}"), name, row)?;
            let child_indent = if last { "    " } else { "│   " };
            render_children(out, children, path, &format!("{indent}{child_indent}"))?;
        }
    }
    Ok(())
}

fn write_row(out: &mut impl Write, branch: &str, name: &str, row: &Value) -> Result<()> {
    let suffix = if row["kind"] == "directory" {
        "/"
    } else if row["kind"] == "symlink" {
        " (symlink)"
    } else {
        ""
    };
    write!(out, "{branch}{}{suffix}", name.escape_debug())?;
    if let Some(bytes) = row["bytes"].as_u64() {
        write!(out, " {}", size(bytes))?;
    }
    writeln!(out)?;
    Ok(())
}
