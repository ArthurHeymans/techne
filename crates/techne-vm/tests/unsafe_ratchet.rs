//! Keep raw heap access from spreading beyond the collector and VM core.

use std::{collections::BTreeMap, path::Path};

/// The ceilings of a budget: `file count` lines, `#` starting a comment.
fn budget(text: &str) -> BTreeMap<String, usize> {
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            let (file, count) = line.split_once(' ').unwrap();
            (file.to_string(), count.parse::<usize>().unwrap())
        })
        .collect()
}

fn counts(dir: &Path, root: &Path, out: &mut BTreeMap<String, usize>) {
    if !dir.exists() {
        return;
    }
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.strip_prefix(root).unwrap().to_str().unwrap().replace('\\', "/");
        if matches!(name.as_str(), "src/heap.rs" | "src/vm.rs" | "src/jit.rs" | "src/alloc.rs")
            || ["src/heap/", "src/vm/", "src/alloc/"].iter().any(|prefix| name.starts_with(prefix))
        {
            continue;
        }
        if path.is_dir() {
            counts(&path, root, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            // Count whole words, including comments and strings. This is a
            // conservative ceiling, not an estimate of executed blocks.
            let text = std::fs::read_to_string(&path).unwrap();
            let count = text.split(|c: char| !c.is_alphanumeric() && c != '_').filter(|word| *word == "unsafe").count();
            if count > 0 {
                out.insert(name, count);
            }
        }
    }
}

#[test]
fn raw_access_does_not_spread() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let ceilings = budget(include_str!("unsafe-budget.txt"));
    // The budget of the revision this one is compared with (CI, for a pull
    // request): no ceiling may rise above it, a new file's being zero.
    if let Some(base) = std::env::var_os("TECHNE_UNSAFE_BASE") {
        let base = budget(&std::fs::read_to_string(base).unwrap());
        for (file, ceiling) in &ceilings {
            let before = base.get(file).copied().unwrap_or(0);
            assert!(*ceiling <= before, "{file}: ceiling raised from {before} to {ceiling}; move raw access into the VM core");
        }
    }
    let mut actual = BTreeMap::new();
    for dir in ["src", "tests", "benches"] {
        counts(&root.join(dir), root, &mut actual);
    }
    for (file, count) in &actual {
        let ceiling = ceilings.get(file).copied().unwrap_or(0);
        assert!(*count <= ceiling, "{file}: {count} occurrences, ceiling {ceiling}; move raw access into the VM core");
    }
    for (file, ceiling) in ceilings {
        let count = actual.get(&file).copied().unwrap_or(0);
        assert_eq!(count, ceiling, "{file}: lower tests/unsafe-budget.txt to {count}");
    }
}
