//! The host's view of a world: its values as `api::Value`s, each checked
//! to be of this world on the way in (`Vm::import_value`) and held through a
//! root on the way out (`Vm::export`). Inside the VM the raw forms of these
//! methods (`*_raw`) are used. What allocates charges this world, which
//! the host may call while another world's account is entered.

use std::{any::Any, path::Path, rc::Rc};

use super::{Error, ErrorKind, ProcedureInfo, Vm};
use crate::{api::Value, reader::Sexp};

impl Vm {
    /// The value of global `name`, an error if unbound.
    pub fn global_value(&mut self, name: &str) -> Result<Value, Error> {
        let _entered = self.charging();
        let v = self.global_value_raw(name)?;
        Ok(self.export(v))
    }

    pub fn get_global(&mut self, name: &str) -> Option<Value> {
        let _entered = self.charging();
        let v = self.get_global_raw(name)?;
        Some(self.export(v))
    }

    pub fn get_global_in(&mut self, module: u32, name: &str) -> Option<Value> {
        let _entered = self.charging();
        let v = self.get_global_in_raw(module, name)?;
        Some(self.export(v))
    }

    pub fn set_global(&mut self, name: &str, v: &Value) -> Result<(), Error> {
        let _charged = self.charging();
        let v = self.import_value(v)?;
        self.set_global_raw(name, v);
        Ok(())
    }

    /// Wrap a Rust value as a Scheme object. It is dropped when the object dies.
    pub fn make_foreign(&mut self, value: Rc<dyn Any>, type_name: &'static str) -> Value {
        let _charged = self.charging();
        let v = self.make_foreign_raw(value, type_name);
        self.export(v)
    }

    /// The Rust value of foreign object `v`, and its type's name.
    pub fn foreign(&self, v: &Value) -> Option<(Rc<dyn Any>, &'static str)> {
        let _entered = self.charging();
        let v = self.import_value(v).ok()?;
        self.foreign_raw(v).map(|(rc, name)| (rc.clone(), name))
    }

    pub fn alloc_pair(&mut self, car: &Value, cdr: &Value) -> Result<Value, Error> {
        let _charged = self.charging();
        let (car, cdr) = (self.import_value(car)?, self.import_value(cdr)?);
        let v = self.alloc_pair_raw(car, cdr);
        Ok(self.export(v))
    }

    pub fn make_string(&mut self, text: &str) -> Value {
        let _charged = self.charging();
        let v = self.make_string_raw(text);
        self.export(v)
    }

    pub fn make_bytevector(&mut self, bytes: &[u8]) -> Value {
        let _charged = self.charging();
        let v = self.make_bytevector_raw(bytes);
        self.export(v)
    }

    pub fn make_vector(&mut self, items: &[Value]) -> Result<Value, Error> {
        let _charged = self.charging();
        let items = self.import_all(items)?;
        let v = self.make_vector_raw(&items);
        Ok(self.export(v))
    }

    pub fn make_list(&mut self, items: &[Value]) -> Result<Value, Error> {
        let _charged = self.charging();
        let items = self.import_all(items)?;
        let v = self.make_list_raw(&items);
        Ok(self.export(v))
    }

    /// An exact integer: a fixnum, or a big integer.
    pub fn make_int(&mut self, i: i64) -> Value {
        let _charged = self.charging();
        let v = self.make_int_raw(i);
        self.export(v)
    }

    pub fn make_error_object(&mut self, message: &str, irritants: &[Value]) -> Result<Value, Error> {
        let _entered = self.charging();
        self.make_error_object_of(message, irritants, ErrorKind::General)
    }

    pub fn make_error_object_of(&mut self, message: &str, irritants: &[Value], kind: ErrorKind) -> Result<Value, Error> {
        let _charged = self.charging();
        let irritants = self.import_all(irritants)?;
        let v = self.make_error_object_of_raw(message, &irritants, kind);
        Ok(self.export(v))
    }

    pub fn eval_source(&mut self, source: &str) -> Result<Value, Error> {
        let _entered = self.charging();
        let v = self.eval_source_raw(source)?;
        Ok(self.export(v))
    }

    pub fn eval_file(&mut self, path: &Path) -> Result<Value, Error> {
        let _entered = self.charging();
        let v = self.eval_file_raw(path)?;
        Ok(self.export(v))
    }

    pub fn eval_interactive(&mut self, module: &mut u32, name: &str, source: &str) -> Result<Value, Error> {
        let _entered = self.charging();
        let v = self.eval_interactive_raw(module, name, source)?;
        Ok(self.export(v))
    }

    pub fn eval_in(&mut self, module: u32, name: &str, source: &str) -> Result<Value, Error> {
        let _entered = self.charging();
        let v = self.eval_in_raw(module, name, source)?;
        Ok(self.export(v))
    }

    pub fn eval_sexp(&mut self, form: &Sexp) -> Result<Value, Error> {
        let _entered = self.charging();
        let v = self.eval_sexp_raw(form)?;
        Ok(self.export(v))
    }

    pub fn eval_sexp_in(&mut self, module: u32, form: &Sexp) -> Result<Value, Error> {
        let _entered = self.charging();
        let v = self.eval_sexp_in_raw(module, form)?;
        Ok(self.export(v))
    }

    /// Call procedure `f` with `args`.
    pub fn call(&mut self, f: &Value, args: &[Value]) -> Result<Value, Error> {
        let _entered = self.charging();
        let f = self.import_value(f)?;
        let args = self.import_all(args)?;
        let v = self.call_raw(f, &args)?;
        Ok(self.export(v))
    }

    pub fn is_procedure(&self, v: &Value) -> bool {
        let _entered = self.charging();
        self.import_value(v).is_ok_and(|v| self.is_procedure_raw(v))
    }

    /// Make `handler` the innermost handler of conditions.
    pub fn push_handler(&mut self, handler: &Value) -> Result<(), Error> {
        let _entered = self.charging();
        let handler = self.import_value(handler)?;
        self.push_handler_raw(handler);
        Ok(())
    }

    /// Call `thunk` with `handler` handling the conditions it raises.
    pub fn with_handler(&mut self, handler: &Value, thunk: &Value) -> Result<Value, Error> {
        let _entered = self.charging();
        let (handler, thunk) = (self.import_value(handler)?, self.import_value(thunk)?);
        let v = self.with_handler_raw(handler, thunk)?;
        Ok(self.export(v))
    }

    pub fn raise_continuable(&mut self, v: &Value) -> Result<Value, Error> {
        let _entered = self.charging();
        let v = self.import_value(v)?;
        let v = self.raise_continuable_raw(v)?;
        Ok(self.export(v))
    }

    /// The error raising `v`.
    pub fn raise_error(&mut self, v: &Value) -> Error {
        let _entered = self.charging();
        match self.import_value(v) {
            Ok(v) => self.raise_error_raw(v),
            Err(e) => e,
        }
    }

    /// The error of `who` given `got`, not `expected`.
    pub fn type_error(&self, who: &str, expected: &str, got: &Value) -> Error {
        let _entered = self.charging();
        match self.import_value(got) {
            Ok(v) => crate::builtins::type_error(who, expected, v),
            Err(e) => e,
        }
    }

    /// The message of condition `v`, its irritants each written briefly.
    pub fn condition_message(&self, v: &Value) -> String {
        let _entered = self.charging();
        match self.import_value(v) {
            Ok(v) => crate::builtins::condition_message(self, v),
            Err(e) => e.msg.clone(),
        }
    }

    pub fn describe_value(&self, name: &str, v: &Value) -> String {
        let _entered = self.charging();
        match self.import_value(v) {
            Ok(v) => self.describe_value_raw(name, v),
            Err(e) => format!("{name}: {}", e.msg),
        }
    }

    pub fn procedure_info(&self, v: &Value) -> Option<ProcedureInfo> {
        let _entered = self.charging();
        self.procedure_info_raw(self.import_value(v).ok()?)
    }

    pub fn documentation(&self, v: &Value) -> Option<Rc<str>> {
        let _entered = self.charging();
        self.documentation_raw(self.import_value(v).ok()?)
    }

    pub fn procedure_name(&self, v: &Value) -> Option<Rc<str>> {
        let _entered = self.charging();
        self.procedure_name_raw(self.import_value(v).ok()?)
    }

    /// Why `target` is still alive (see `why_retained_raw`).
    pub fn why_retained(&self, target: &Value, asking: std::ops::Range<usize>) -> Option<Vec<String>> {
        let _entered = self.charging();
        self.why_retained_raw(self.import_value(target).ok()?, asking)
    }
}
