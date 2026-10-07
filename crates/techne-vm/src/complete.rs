//! Completion of names, as a service of the language rather than of one
//! interface: the REPL, the nREPL server and the editor (`module-completions`)
//! all ask here what a module can see and where the identifier being typed
//! starts. Filtering the names by what was typed is the interface's: the
//! REPL by prefix, the editor as its completion style says.

use std::rc::Rc;

use crate::{
    compiler::is_special_form,
    reader,
    vm::{GlobalBinding, Vm},
};

/// What a name is bound to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A special form of the compiler.
    Syntax,
    Macro,
    Procedure,
    Variable,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Syntax => "syntax",
            Kind::Macro => "macro",
            Kind::Procedure => "procedure",
            Kind::Variable => "variable",
        }
    }
}

/// Whether `c` can be part of an identifier: not whitespace and not a
/// character the reader ends one at.
pub fn identifier_char(c: char) -> bool {
    !c.is_whitespace() && !"()[]{}'\"`,;".contains(c)
}

/// Where the identifier ending at byte `pos` of `text` starts: completion
/// replaces the text from there to `pos`.
pub fn identifier_start(text: &str, pos: usize) -> usize {
    text[..pos].rfind(|c: char| !identifier_char(c)).map_or(0, |i| i + text[i..].chars().next().map_or(1, char::len_utf8))
}

impl Vm {
    /// The names `module` sees (its own, those it imports, the root
    /// module's and the special forms), sorted, each with its kind.
    pub fn completions(&self, module: u32) -> Vec<(Rc<str>, Kind)> {
        self.global_names(module)
            .into_iter()
            .map(|name| {
                let kind = if is_special_form(&name) {
                    Kind::Syntax
                } else {
                    match self.lookup_global(module, reader::intern(&name)) {
                        Some(GlobalBinding::Macro(_)) => Kind::Macro,
                        Some(GlobalBinding::Var(g)) if self.is_procedure(self.globals[g as usize]) => Kind::Procedure,
                        _ => Kind::Variable,
                    }
                };
                (name, kind)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_end_where_the_reader_ends_them() {
        assert_eq!(identifier_start("(string-app", 11), 1);
        assert_eq!(identifier_start("'λx", "'λx".len()), 1);
        assert_eq!(identifier_start("car", 3), 0);
        assert_eq!(identifier_start("(f ", 3), 3);
    }

    #[test]
    fn a_module_s_names_and_their_kinds() {
        let mut vm = Vm::new();
        vm.eval_source("(define (greet) 1) (define answer 42) (define-syntax swap! (syntax-rules () ((_ a b) #f)))").unwrap();
        let names = vm.completions(crate::vm::USER_MODULE);
        let kind = |n: &str| names.iter().find(|(m, _)| &**m == n).map(|(_, k)| *k);
        assert_eq!(
            [kind("greet"), kind("answer"), kind("swap!"), kind("if"), kind("car")],
            [Some(Kind::Procedure), Some(Kind::Variable), Some(Kind::Macro), Some(Kind::Syntax), Some(Kind::Procedure)]
        );
        assert!(names.iter().all(|(n, _)| !n.starts_with('%')));
    }
}
