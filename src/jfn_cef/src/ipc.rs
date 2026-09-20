use cef::{ImplListValue, ListValue, sys};

pub(crate) struct BrowserMessage {
    name: String,
    args: Option<ListValue>,
}

impl BrowserMessage {
    pub(crate) fn new(name: String, args: Option<ListValue>) -> Self {
        Self { name, args }
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn args(&self) -> Option<&ListValue> {
        self.args.as_ref()
    }
}

pub(crate) fn list_string(args: &ListValue, idx: usize) -> String {
    crate::cef_string::userfree_to_string(&args.string(idx))
}

pub(crate) fn list_opt_string(args: &ListValue, idx: usize) -> Option<String> {
    if args.get_type(idx).as_ref() == &sys::cef_value_type_t::VTYPE_STRING {
        Some(list_string(args, idx))
    } else {
        None
    }
}

/// JS can send integers as `VTYPE_DOUBLE` (e.g. via `parseFloat`); round to i32 in that case.
pub(crate) fn list_int(args: &ListValue, idx: usize) -> i32 {
    let t = args.get_type(idx);
    if t.as_ref() == &sys::cef_value_type_t::VTYPE_DOUBLE {
        args.double(idx).round() as i32
    } else {
        args.int(idx)
    }
}

/// Mirror of [`list_int`] for the other direction: V8 marshals a whole-number JS
/// value (e.g. `2000 / 1000 == 2`) as `VTYPE_INT`, and `ListValue::double` returns
/// `0.0` for a non-double-typed slot. Read it as an int and widen, so integral
/// values (a 2.0s subtitle delay, etc.) aren't silently dropped to zero.
pub(crate) fn list_double(args: &ListValue, idx: usize) -> f64 {
    let t = args.get_type(idx);
    if t.as_ref() == &sys::cef_value_type_t::VTYPE_INT {
        f64::from(args.int(idx))
    } else {
        args.double(idx)
    }
}
