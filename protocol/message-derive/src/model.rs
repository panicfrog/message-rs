//! Rust type to layout-category classification and `#[msg(...)]` attribute parsing.

use quote::quote;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarKind {
    Bool,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    F32,
    F64,
}

impl ScalarKind {
    pub fn width(self) -> usize {
        match self {
            ScalarKind::Bool | ScalarKind::U8 | ScalarKind::I8 => 1,
            ScalarKind::U16 | ScalarKind::I16 => 2,
            ScalarKind::U32 | ScalarKind::I32 | ScalarKind::F32 => 4,
            ScalarKind::U64 | ScalarKind::I64 | ScalarKind::F64 => 8,
        }
    }
}

#[derive(Clone, Debug)]
pub enum TyKind {
    Scalar(ScalarKind),
    Str,
    Bytes,
    Array { elem: Box<TyKind>, fixed: Option<u32> },
    Map { key: Box<TyKind>, value: Box<TyKind> },
    /// A referenced named type (struct or enum; resolved symbolically via `<T as Fixed>` consts).
    Path(syn::Type),
}

pub fn classify(ty: &syn::Type) -> Result<TyKind, syn::Error> {
    match ty {
        syn::Type::Path(p) => {
            if let Some(seg) = p.path.segments.last() {
                let id = seg.ident.to_string();
                match (id.as_str(), &seg.arguments) {
                    ("bool", syn::PathArguments::None) => return Ok(TyKind::Scalar(ScalarKind::Bool)),
                    ("u8", syn::PathArguments::None) => return Ok(TyKind::Scalar(ScalarKind::U8)),
                    ("i8", syn::PathArguments::None) => return Ok(TyKind::Scalar(ScalarKind::I8)),
                    ("u16", syn::PathArguments::None) => return Ok(TyKind::Scalar(ScalarKind::U16)),
                    ("i16", syn::PathArguments::None) => return Ok(TyKind::Scalar(ScalarKind::I16)),
                    ("u32", syn::PathArguments::None) => return Ok(TyKind::Scalar(ScalarKind::U32)),
                    ("i32", syn::PathArguments::None) => return Ok(TyKind::Scalar(ScalarKind::I32)),
                    ("u64", syn::PathArguments::None) => return Ok(TyKind::Scalar(ScalarKind::U64)),
                    ("i64", syn::PathArguments::None) => return Ok(TyKind::Scalar(ScalarKind::I64)),
                    ("f32", syn::PathArguments::None) => return Ok(TyKind::Scalar(ScalarKind::F32)),
                    ("f64", syn::PathArguments::None) => return Ok(TyKind::Scalar(ScalarKind::F64)),
                    ("String", syn::PathArguments::None) => return Ok(TyKind::Str),
                    ("Vec", syn::PathArguments::AngleBracketed(args)) => {
                        let elem_ty = single_arg(args)?;
                        let elem = classify(&elem_ty)?;
                        if matches!(elem, TyKind::Scalar(ScalarKind::U8)) {
                            return Ok(TyKind::Bytes);
                        }
                        return Ok(TyKind::Array { elem: Box::new(elem), fixed: None });
                    }
                    ("HashMap" | "BTreeMap", syn::PathArguments::AngleBracketed(args)) => {
                        let mut iter = args.args.iter();
                        let (k, v) = (first_type(&mut iter)?, first_type(&mut iter)?);
                        return Ok(TyKind::Map { key: Box::new(classify(&k)?), value: Box::new(classify(&v)?) });
                    }
                    _ => {}
                }
            }
            Ok(TyKind::Path(ty.clone()))
        }
        syn::Type::Array(arr) => {
            let elem = classify(&arr.elem)?;
            let n = match &arr.len {
                syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(i), .. }) => {
                    i.base10_parse::<u32>().ok()
                }
                _ => None,
            }
            .ok_or_else(|| syn::Error::new_spanned(&arr.len, "fixed-array length must be an integer literal"))?;
            Ok(TyKind::Array { elem: Box::new(elem), fixed: Some(n) })
        }
        _ => Err(syn::Error::new_spanned(ty, "unsupported field type")),
    }
}

fn single_arg(args: &syn::AngleBracketedGenericArguments) -> Result<syn::Type, syn::Error> {
    first_type(&mut args.args.iter())
}

fn first_type<'a, I: Iterator<Item = &'a syn::GenericArgument>>(
    iter: &mut I,
) -> Result<syn::Type, syn::Error> {
    for arg in iter {
        if let syn::GenericArgument::Type(t) = arg {
            return Ok(t.clone());
        }
    }
    Err(syn::Error::new(proc_macro2::Span::call_site(), "missing generic argument"))
}

/// Whether the element/field shape is slot-shaped (string/bytes/map/dynamic array).
pub fn is_slot_shaped(k: &TyKind) -> bool {
    match k {
        TyKind::Scalar(_) => false,
        TyKind::Str | TyKind::Bytes | TyKind::Map { .. } => true,
        TyKind::Array { elem, fixed } => match fixed {
            Some(_) => is_slot_shaped(elem),
            None => true,
        },
        TyKind::Path(_) => false,
    }
}

/// String form of a syn type (used to build symbolic constant paths).
pub fn ty_str(p: &syn::Type) -> String {
    quote! { #p }.to_string().replace(' ', "")
}

/// Field footprint expression in the fixed region (numeric literal or `<T as Fixed>::FIXED_LEN`).
pub fn size_str(k: &TyKind) -> String {
    match k {
        TyKind::Scalar(s) => format!("{}usize", s.width()),
        TyKind::Str | TyKind::Bytes | TyKind::Map { .. } => "8usize".to_string(),
        TyKind::Array { elem, fixed: Some(n) } if !is_slot_shaped(elem) => {
            format!("({n} * {})", size_str(elem))
        }
        TyKind::Array { .. } => "8usize".to_string(),
        TyKind::Path(p) => format!("<{} as ::message_core::Fixed>::FIXED_LEN", ty_str(p)),
    }
}

/// Alignment expression.
pub fn align_str(k: &TyKind) -> String {
    match k {
        TyKind::Scalar(s) => format!("{}usize", s.width()),
        TyKind::Path(p) => format!("<{} as ::message_core::Fixed>::MAX_ALIGN", ty_str(p)),
        TyKind::Array { elem, fixed: Some(_) } if !is_slot_shaped(elem) => align_str(elem),
        _ => "4usize".to_string(),
    }
}

/// Element stride expression for slot-based arrays.
pub fn element_size_str(k: &TyKind) -> Result<String, syn::Error> {
    Ok(match k {
        TyKind::Scalar(s) => format!("{}usize", s.width()),
        TyKind::Str | TyKind::Bytes => "8usize".to_string(),
        TyKind::Map { .. } => {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "map cannot be used as an array/map element type in V1",
            ))
        }
        TyKind::Path(p) => format!("<{} as ::message_core::Fixed>::FIXED_LEN", ty_str(p)),
        TyKind::Array { elem, fixed: Some(n) } => {
            format!("({n} * {})", element_size_str(elem)?)
        }
        TyKind::Array { elem, fixed: None } => {
            element_size_str(elem)?;
            "8usize".to_string()
        }
    })
}

/// Type expression inside IR descriptors.
pub fn type_expr_str(k: &TyKind) -> String {
    match k {
        TyKind::Scalar(s) => {
            let v = match s {
                ScalarKind::Bool => "Bool",
                ScalarKind::U8 => "U8",
                ScalarKind::I8 => "I8",
                ScalarKind::U16 => "U16",
                ScalarKind::I16 => "I16",
                ScalarKind::U32 => "U32",
                ScalarKind::I32 => "I32",
                ScalarKind::U64 => "U64",
                ScalarKind::I64 => "I64",
                ScalarKind::F32 => "F32",
                ScalarKind::F64 => "F64",
            };
            format!("::message_core::Type::Simple(::message_core::SimpleType::{v})")
        }
        TyKind::Str => "::message_core::Type::Simple(::message_core::SimpleType::String)".to_string(),
        TyKind::Bytes => "::message_core::Type::Simple(::message_core::SimpleType::Bytes)".to_string(),
        TyKind::Path(p) => format!(
            "::message_core::Type::structure({:?})",
            path_last_name(p)
        ),
        TyKind::Array { elem, fixed } => {
            let e = type_expr_str(elem);
            match fixed {
                Some(n) => format!("::message_core::Type::array({e}, ::core::option::Option::Some({n}u32))"),
                None => format!("::message_core::Type::vec({e})"),
            }
        }
        TyKind::Map { key, value } => format!(
            "::message_core::Type::map({}, {})",
            type_expr_str(key),
            type_expr_str(value)
        ),
    }
}

fn path_last_name(p: &syn::Type) -> String {
    if let syn::Type::Path(tp) = p {
        tp.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default()
    } else {
        String::new()
    }
}
