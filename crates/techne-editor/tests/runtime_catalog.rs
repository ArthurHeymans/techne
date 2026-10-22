//! The language server's runtime names, without linking the runtime hosts.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use techne_vm::vm::{USER_MODULE, Vm};

#[derive(Serialize)]
struct Binding {
    description: String,
    is_macro: bool,
}

fn descriptions(vm: &mut Vm) -> BTreeMap<String, String> {
    vm.global_names(USER_MODULE).into_iter().map(|name| (name.to_string(), vm.describe_binding(USER_MODULE, &name))).collect()
}

/// Record only what the hosts add or change. The server gets the plain
/// language's descriptions from its own VM.
fn catalog() -> String {
    let mut vm = Vm::new();
    let language = descriptions(&mut vm);
    let roots: BTreeSet<_> = vm.root_names().into_iter().map(|n| n.to_string()).collect();
    techne_node::install(&mut vm).unwrap();
    techne_editor::install(&mut vm);
    let api = techne_editor::runtime::lisp_dir().join("api.scm");
    vm.eval_source(&format!("(require {:?})", api.display().to_string())).unwrap();
    // Source locations must not depend on the checkout or installation.
    let lisp = techne_vm::library::lisp_dir().canonicalize().unwrap().display().to_string();
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().parent().unwrap().display().to_string();
    let bindings: BTreeMap<_, _> = descriptions(&mut vm)
        .into_iter()
        .filter(|(name, desc)| language.get(name) != Some(desc))
        .map(|(name, description)| {
            (
                name.clone(),
                Binding {
                    is_macro: vm.describe_name(USER_MODULE, &name).kind == "macro",
                    description: description.replace(&lisp, "@lisp@").replace(&source, "@source@"),
                },
            )
        })
        .collect();
    let root_names: BTreeSet<_> = vm.root_names().into_iter().map(|n| n.to_string()).filter(|n| !roots.contains(n)).collect();
    format!("{}\n", serde_json::to_string_pretty(&serde_json::json!({ "bindings": bindings, "root_names": root_names })).unwrap())
}

#[test]
fn runtime_catalog_is_current() {
    let path = techne_vm::library::lisp_dir().join("runtime-catalog.json");
    let generated = catalog();
    if std::env::var_os("TECHNE_UPDATE_CATALOG").is_some() {
        std::fs::write(&path, &generated).unwrap();
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == generated,
        "{} is stale; refresh with: TECHNE_UPDATE_CATALOG=1 cargo test --release -p techne-editor --test runtime_catalog",
        path.display()
    );
}
