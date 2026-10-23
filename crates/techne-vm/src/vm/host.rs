//! The host's view of a world: its values as `api::Value`s, each checked
//! to be of this world on the way in (`Vm::import_value`) and held through a
//! root on the way out (`Vm::export`). Inside the VM the raw forms of these
//! methods (`*_raw`) are used. Each goes through the host's boundary
//! (`Vm::host`): its world entered, whichever the host is in, and a panic
//! retiring the world, not unwinding into the host.

use std::{any::Any, path::Path, rc::Rc};

use super::{Error, ErrorKind, Handler, ProcedureInfo, USER_MODULE, Vm};
use crate::{api::Value, reader::Sexp};

impl Vm {
    /// The value of global `name`, an error if unbound.
    pub fn global_value(&mut self, name: &str) -> Result<Value, Error> {
        self.host_try(|vm| vm.global_value_raw(name).map(|v| vm.export(v)))
    }

    pub fn get_global(&mut self, name: &str) -> Option<Value> {
        self.host(|vm| vm.get_global_raw(name).map(|v| vm.export(v))).ok().flatten()
    }

    pub fn get_global_in(&mut self, module: u32, name: &str) -> Option<Value> {
        self.host(|vm| vm.get_global_in_raw(module, name).map(|v| vm.export(v))).ok().flatten()
    }

    pub fn set_global(&mut self, name: &str, v: &Value) -> Result<(), Error> {
        self.host_try(|vm| {
            let v = vm.import_value(v)?;
            vm.set_global_raw(name, v);
            Ok(())
        })
    }

    /// Wrap a Rust value as a Scheme object. It is dropped when the object dies.
    pub fn make_foreign(&mut self, value: Rc<dyn Any>, type_name: &'static str) -> Result<Value, Error> {
        self.host(|vm| {
            let v = vm.make_foreign_raw(value, type_name);
            vm.export(v)
        })
    }

    /// The Rust value of foreign object `v`, and its type's name.
    pub fn foreign(&self, v: &Value) -> Option<(Rc<dyn Any>, &'static str)> {
        self.host_ref(|vm| vm.foreign_raw(vm.import_value(v).ok()?).map(|(rc, name)| (rc.clone(), name))).ok().flatten()
    }

    pub fn alloc_pair(&mut self, car: &Value, cdr: &Value) -> Result<Value, Error> {
        self.host_try(|vm| {
            let (car, cdr) = (vm.import_value(car)?, vm.import_value(cdr)?);
            let v = vm.alloc_pair_raw(car, cdr);
            Ok(vm.export(v))
        })
    }

    pub fn make_string(&mut self, text: &str) -> Result<Value, Error> {
        self.host(|vm| {
            let v = vm.make_string_raw(text);
            vm.export(v)
        })
    }

    pub fn make_bytevector(&mut self, bytes: &[u8]) -> Result<Value, Error> {
        self.host(|vm| {
            let v = vm.make_bytevector_raw(bytes);
            vm.export(v)
        })
    }

    pub fn make_vector(&mut self, items: &[Value]) -> Result<Value, Error> {
        self.host_try(|vm| {
            let items = vm.import_all(items)?;
            let v = vm.make_vector_raw(&items);
            Ok(vm.export(v))
        })
    }

    pub fn make_list(&mut self, items: &[Value]) -> Result<Value, Error> {
        self.host_try(|vm| {
            let items = vm.import_all(items)?;
            let v = vm.make_list_raw(&items);
            Ok(vm.export(v))
        })
    }

    /// An exact integer: a fixnum, or a big integer.
    pub fn make_int(&mut self, i: i64) -> Result<Value, Error> {
        self.host(|vm| {
            let v = vm.make_int_raw(i);
            vm.export(v)
        })
    }

    pub fn make_error_object(&mut self, message: &str, irritants: &[Value]) -> Result<Value, Error> {
        self.make_error_object_of(message, irritants, ErrorKind::General)
    }

    pub fn make_error_object_of(&mut self, message: &str, irritants: &[Value], kind: ErrorKind) -> Result<Value, Error> {
        self.host_try(|vm| {
            let irritants = vm.import_all(irritants)?;
            let v = vm.make_error_object_of_raw(message, &irritants, kind);
            Ok(vm.export(v))
        })
    }

    pub fn eval_source(&mut self, source: &str) -> Result<Value, Error> {
        self.eval_in(USER_MODULE, "<input>", source)
    }

    pub fn eval_file(&mut self, path: &Path) -> Result<Value, Error> {
        self.host_try(|vm| {
            if !vm.input_ready() {
                return vm.eval_file_raw(path).map(|v| vm.export(v));
            }
            let text = std::fs::read_to_string(path).map_err(|e| Error::new(format!("{}: {e}", path.display())))?;
            let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
            vm.modules[USER_MODULE as usize].path = Some(canonical.clone());
            vm.module_paths.insert(canonical, USER_MODULE);
            let mut module = USER_MODULE;
            vm.eval_input_source(&mut module, &path.to_string_lossy(), &text)
        })
    }

    pub fn eval_interactive(&mut self, module: &mut u32, name: &str, source: &str) -> Result<Value, Error> {
        self.host_try(|vm| {
            if vm.input_ready() {
                vm.eval_input_source(module, name, source)
            } else {
                vm.eval_interactive_raw(module, name, source).map(|v| vm.export(v))
            }
        })
    }

    pub fn eval_in(&mut self, module: u32, name: &str, source: &str) -> Result<Value, Error> {
        self.host_try(|vm| {
            if vm.input_ready() {
                let mut selected = module;
                vm.eval_input_source(&mut selected, name, source)
            } else {
                vm.eval_in_raw(module, name, source).map(|v| vm.export(v))
            }
        })
    }

    pub fn eval_sexp(&mut self, form: &Sexp) -> Result<Value, Error> {
        self.eval_sexp_in(self.current_module(), form)
    }

    pub fn eval_sexp_in(&mut self, module: u32, form: &Sexp) -> Result<Value, Error> {
        self.host_try(|vm| {
            if vm.input_ready() { vm.eval_input_sexp(module, form) } else { vm.eval_sexp_in_raw(module, form).map(|v| vm.export(v)) }
        })
    }

    /// Call procedure `f` with `args`.
    pub fn call(&mut self, f: &Value, args: &[Value]) -> Result<Value, Error> {
        self.host_try(|vm| {
            let f = vm.import_value(f)?;
            let args = vm.import_all(args)?;
            vm.call_host(f, &args)
        })
    }

    /// Call as a task at an idle host boundary, never as a native callback.
    pub fn call_input(&mut self, f: &Value, args: &[Value]) -> Result<Value, Error> {
        if !self.input_ready() {
            return Err(Error::new("an input requires an idle host boundary"));
        }
        self.call(f, args)
    }

    pub fn is_procedure(&self, v: &Value) -> bool {
        self.host_ref(|vm| vm.import_value(v).is_ok_and(|v| vm.is_procedure_raw(v))).unwrap_or(false)
    }

    /// Make `handler` the innermost handler of conditions.
    pub fn push_handler(&mut self, handler: &Value) -> Result<(), Error> {
        self.host_try(|vm| {
            let handler = vm.import_value(handler)?;
            vm.push_handler_raw(handler);
            Ok(())
        })
    }

    /// Call `thunk` with `handler` handling the conditions it raises.
    pub fn with_handler(&mut self, handler: &Value, thunk: &Value) -> Result<Value, Error> {
        self.host_try(|vm| {
            let (handler, thunk) = (vm.import_value(handler)?, vm.import_value(thunk)?);
            // This is host entry, like `call`: an adapter invoking it
            // from a native necessarily nests dispatch. Guest handlers
            // instead use frames in the raising task (`call_handler`).
            let handler = vm.root(handler);
            vm.handlers.push(Handler::Proc { handler });
            let depth = vm.handlers.len();
            let result = vm.call_host(thunk, &[]);
            // Guest winds have already completed in the dispatcher.
            vm.handlers.truncate(depth - 1);
            if vm.poisoned().is_some() {
                return Err(vm.poison_error());
            }
            result
        })
    }

    pub fn raise_continuable(&mut self, v: &Value) -> Result<Value, Error> {
        self.host_try(|vm| {
            let v = vm.import_value(v)?;
            // Only a host call runs a handler synchronously. Scheme's
            // raise-continuable calls it on the task's own VM stack.
            let Some(i) = vm.continuable_handler() else { return Err(vm.raise_error_raw(v)) };
            let handler = vm.handler_at(i).expect("a handler procedure");
            let depth = vm.handlers.len();
            vm.handlers.push(Handler::Mask { lo: i, raised: None });
            let result = vm.call_host(handler, &[v]);
            vm.handlers.truncate(depth);
            result
        })
    }

    /// The error raising `v`.
    pub fn raise_error(&mut self, v: &Value) -> Error {
        self.host(|vm| match vm.import_value(v) {
            Ok(v) => vm.raise_error_raw(v),
            Err(e) => e,
        })
        .unwrap_or_else(|e| e)
    }

    /// The error of `who` given `got`, not `expected`.
    pub fn type_error(&self, who: &str, expected: &str, got: &Value) -> Error {
        self.host_ref(|vm| match vm.import_value(got) {
            Ok(v) => crate::builtins::type_error(who, expected, v),
            Err(e) => e,
        })
        .unwrap_or_else(|e| e)
    }

    /// The message of condition `v`, its irritants each written briefly.
    pub fn condition_message(&self, v: &Value) -> String {
        self.host_ref(|vm| match vm.import_value(v) {
            Ok(v) => crate::builtins::condition_message(vm, v),
            Err(e) => e.msg.clone(),
        })
        .unwrap_or_else(|e| e.msg.clone())
    }

    pub fn describe_value(&self, name: &str, v: &Value) -> String {
        self.host_ref(|vm| match vm.import_value(v) {
            Ok(v) => vm.describe_value_raw(name, v),
            Err(e) => format!("{name}: {}", e.msg),
        })
        .unwrap_or_else(|e| format!("{name}: {}", e.msg))
    }

    pub fn procedure_info(&self, v: &Value) -> Option<ProcedureInfo> {
        self.host_ref(|vm| vm.procedure_info_raw(vm.import_value(v).ok()?)).ok().flatten()
    }

    pub fn documentation(&self, v: &Value) -> Option<Rc<str>> {
        self.host_ref(|vm| vm.documentation_raw(vm.import_value(v).ok()?)).ok().flatten()
    }

    pub fn procedure_name(&self, v: &Value) -> Option<Rc<str>> {
        self.host_ref(|vm| vm.procedure_name_raw(vm.import_value(v).ok()?)).ok().flatten()
    }

    /// Why `target` is still alive (see `why_retained_raw`).
    pub fn why_retained(&self, target: &Value, asking: std::ops::Range<usize>) -> Option<Vec<String>> {
        self.host_ref(|vm| vm.why_retained_raw(vm.import_value(target).ok()?, asking)).ok().flatten()
    }
}
