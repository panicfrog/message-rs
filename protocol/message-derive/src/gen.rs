//! Expansion of `#[derive(Message)]` / `#[derive(MessageEnum)]`.
//!
//! Emits: layout constants (`__Layout` + const fn), `encode` (header + base + extension
//! block chain + trailers), a zero-copy Reader (lazy per-field access + `to_owned`), map views,
//! the descriptor and the inventory registration. The generator builds Rust source strings and
//! parses them, which keeps the output easy to inspect.

use crate::model::*;
use std::collections::BTreeMap;
use std::fmt::Write as _;

pub struct FieldSpec {
    pub name: String,
    pub ty: syn::Type,
    pub kind: TyKind,
    pub optional: bool,
    pub since: u16,
    pub precise: bool,
    pub reserved: bool,
    pub doc: Option<String>,
}

pub fn expand_struct(input: &syn::DeriveInput) -> Result<proc_macro2::TokenStream, syn::Error> {
    let name = input.ident.to_string();
    let attrs = parse_container_attrs(&input.attrs)?;
    let key = attrs.key.ok_or_else(|| {
        syn::Error::new_spanned(&input.ident, format!("`{name}` is missing the #[msg(key = N)] attribute"))
    })?;

    let fields_named = match &input.data {
        syn::Data::Struct(syn::DataStruct { fields: syn::Fields::Named(f), .. }) => f,
        _ => return Err(syn::Error::new_spanned(&input.ident, "Message only supports named structs")),
    };

    let mut specs: Vec<FieldSpec> = Vec::new();
    for f in &fields_named.named {
        let (inner_ty, optional) = match &f.ty {
            syn::Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "Option") => {
                let args = match &p.path.segments.last().unwrap().arguments {
                    syn::PathArguments::AngleBracketed(a) => a,
                    _ => return Err(syn::Error::new_spanned(&f.ty, "Option needs generic arguments")),
                };
                let inner = args
                    .args
                    .iter()
                    .find_map(|a| match a {
                        syn::GenericArgument::Type(t) => Some(t.clone()),
                        _ => None,
                    })
                    .ok_or_else(|| syn::Error::new_spanned(&f.ty, "Option needs a type argument"))?;
                (inner, true)
            }
            _ => (f.ty.clone(), false),
        };
        let kind = classify(&inner_ty)?;
        let mut spec = FieldSpec {
            name: f.ident.clone().unwrap().to_string(),
            ty: inner_ty,
            kind,
            optional,
            since: 1,
            precise: false,
            reserved: false,
            doc: None,
        };
        parse_field_attrs(&mut spec, &f.attrs)?;
        check_kind_supported(&spec)?;
        specs.push(spec);
    }

    // ---- Per-version blocks + optional bit indices ----
    let mut blocks: BTreeMap<u16, Vec<usize>> = BTreeMap::new();
    for (i, s) in specs.iter().enumerate() {
        blocks.entry(s.since).or_default().push(i);
    }
    let mut opt_bits: Vec<Option<usize>> = vec![None; specs.len()];
    for idxs in blocks.values() {
        let mut bit = 0usize;
        for &i in idxs {
            if specs[i].optional {
                opt_bits[i] = Some(bit);
                bit += 1;
            }
        }
    }
    let ext_versions: Vec<u16> = blocks.keys().copied().filter(|v| *v > 1).collect();
    let max_version: u8 = ext_versions.last().copied().unwrap_or(0) as u8;

    // ---- Layout constants ----
    let layout_ty = format!("__{name}Layout");
    let mut lf: Vec<String> = Vec::new();
    let mut lb: Vec<String> = Vec::new();
    let mut li: Vec<String> = Vec::new();

    lf.push("pub base_bitmap_len: usize".into());
    lf.push("pub base_fixed_len: usize".into());
    lf.push("pub base_max_align: usize".into());

    for (&version, idxs) in &blocks {
        let optional_count = idxs.iter().filter(|i| specs[**i].optional).count();
        let bitmap_len = bitmap_bytes(optional_count);
        let mut aligns: Vec<String> = vec!["1usize".into()];
        if bitmap_len > 0 {
            aligns.push("4usize".into());
        }
        if version == 1 {
            lb.push(format!("let base_bitmap_len: usize = {bitmap_len}usize;"));
            lb.push("let mut c: usize = base_bitmap_len;".into());
            li.push("base_bitmap_len".into());
            li.push("base_fixed_len".into());
            li.push("base_max_align".into());
        } else {
            let bl = format!("b{version}_bitmap_len");
            lf.push(format!("pub {bl}: usize"));
            lb.push(format!("let {bl}: usize = {bitmap_len}usize;"));
            lb.push(format!("let mut c: usize = {bl};"));
            li.push(bl);
        }
        for &i in idxs {
            let s = &specs[i];
            let fdef = layout_field(&s.name);
            let sz = size_str(&s.kind);
            let al = align_str(&s.kind);
            aligns.push(al.clone());
            lf.push(format!("pub {fdef}: usize"));
            lb.push(format!("let {fdef}: usize = ::message_core::align_up_usize(c, {al});"));
            lb.push(format!("c = {fdef} + {sz};"));
            li.push(fdef);
        }
        let max_chain = fold_max(&aligns, &layout_ty);
        if version == 1 {
            lb.push(format!("let base_max_align: usize = {max_chain};"));
            lb.push(format!(
                "let base_fixed_len: usize = ::message_core::align_up_usize(c, base_max_align);"
            ));
        } else {
            let fl = format!("b{version}_fixed_len");
            lf.push(format!("pub {fl}: usize"));
            lb.push(format!(
                "let {fl}: usize = ::message_core::align_up_usize(c, {max_chain});"
            ));
            li.push(fl);
        }
    }
    for s in &specs {
        if let TyKind::Map { key, value } = &s.kind {
            let base = format!("m_{}", s.name);
            let (vo0, st0) = (format!("{base}_v_off0"), format!("{base}_stride0"));
            let (vo1, st1) = (format!("{base}_v_off1"), format!("{base}_stride1"));
            let k_sz = size_str(key);
            let k_al = align_str(key);
            // Open pair: the value inlined at its natural alignment (a struct's fixed region).
            // Instance pair: the value is one 8-byte slot pointing at a self-contained instance.
            let (v_sz, v_al) = match value.as_ref() {
                TyKind::Path(p) => (
                    format!("<{} as ::message_core::Fixed>::FIXED_LEN", ty_str(p)),
                    format!("<{} as ::message_core::Fixed>::MAX_ALIGN", ty_str(p)),
                ),
                _ => (size_str(value), align_str(value)),
            };
            lf.push(format!("pub {vo0}: usize"));
            lf.push(format!("pub {st0}: usize"));
            lf.push(format!("pub {vo1}: usize"));
            lf.push(format!("pub {st1}: usize"));
            lb.push(format!("let {vo0}: usize = ::message_core::align_up_usize({k_sz}, {v_al});"));
            lb.push(format!(
                "let {st0}: usize = ::message_core::align_up_usize({vo0} + {v_sz}, {layout_ty}::__max({k_al}, {v_al}));"
            ));
            lb.push(format!("let {vo1}: usize = ::message_core::align_up_usize({k_sz}, 4usize);"));
            lb.push(format!(
                "let {st1}: usize = ::message_core::align_up_usize({vo1} + 8usize, {layout_ty}::__max({k_al}, 4usize));"
            ));
            li.push(vo0);
            li.push(st0);
            li.push(vo1);
            li.push(st1);
        }
    }

    let reader_ty = format!("{name}Reader");

    // ---- encode ----
    let mut base_writes = String::new();
    if let Some(idxs) = blocks.get(&1) {
        for &i in idxs {
            let _ = writeln!(base_writes, "{}", emit_field_write(&specs[i], opt_bits[i], "origin", "origin"));
        }
    }
    let mut ext_writes = String::new();
    for &v in &ext_versions {
        let idxs = &blocks[&v];
        let mut fw = String::new();
        for &i in idxs {
            let _ = writeln!(fw, "{}", emit_field_write(&specs[i], opt_bits[i], "__blk", "__blk"));
        }
        let _ = writeln!(
            ext_writes,
            "{{\n    let __blk = buf.reserve(__L.b{v}_fixed_len);\n{fw}    let __end = buf.payload_len();\n    let __blen = __end - buf.payload_rel(__blk) + 6usize;\n    let __t = buf.reserve(6usize);\n    buf.write_u16(__t, {v}u16);\n    buf.write_u32(__t + 2usize, __blen as u32);\n}}"
        );
    }

    // ---- Accessors / to_owned / map views ----
    let mut accessors = String::new();
    for (i, s) in specs.iter().enumerate() {
        if s.reserved {
            continue;
        }
        let _ = writeln!(accessors, "{}", emit_accessor(s, &name, opt_bits[i]));
    }
    let mut owned_inits = String::new();
    for s in &specs {
        let _ = writeln!(owned_inits, "    {}: {},", s.name, owned_field_expr(s));
    }
    let mut map_views = String::new();
    for s in &specs {
        if s.reserved {
            continue;
        }
        if let TyKind::Map { key, value } = &s.kind {
            map_views.push_str(&emit_map_view(s, key, value, &name));
        }
    }

    // ---- validate ----
    let mut base_checks = String::new();
    if let Some(idxs) = blocks.get(&1) {
        for &i in idxs {
            if !specs[i].reserved {
                base_checks.push_str(&emit_field_validate(&specs[i], opt_bits[i], "origin"));
            }
        }
    }
    let mut ext_checks = String::new();
    // Highest version first: a corrupt tail is reported at the block that owns it. Two modes:
    // top-level / open (hv = the writer's header version, the block is promised) and instance
    // (hv = INSTANCE_HV, no promise: check the block only when the chain offers it).
    for &v in ext_versions.iter().rev() {
        let mut body = String::new();
        for &i in &blocks[&v] {
            if !specs[i].reserved {
                body.push_str(&emit_field_validate(&specs[i], opt_bits[i], "__bs"));
            }
        }
        ext_checks.push_str(&format!(
            "if hv == ::message_core::INSTANCE_HV {{\n    if let ::core::option::Option::Some((__bs, _)) = ::message_core::find_ext_block_bounded(payload, <{name} as ::message_core::Message>::EXT_VERSIONS, hv, {v}u16, __L.b{v}_fixed_len, origin, end) {{\n    {body}\n    }}\n}} else if hv >= {v}u8 {{\n    let (__bs, _) = ::message_core::check_ext_block(payload, <{name} as ::message_core::Message>::EXT_VERSIONS, hv, {v}u16, __L.b{v}_fixed_len)?;\n    {body}\n}}\n"
        ));
    }

    // ---- Descriptor ----
    let mut field_descs = String::new();
    for (i, s) in specs.iter().enumerate() {
        let _ = writeln!(
            field_descs,
            "        ::message_core::FieldDescriptor {{ {} }},",
            field_desc_body(s, opt_bits[i])
        );
    }
    let mut ext_block_descs = String::new();
    for &v in &ext_versions {
        let bitmap_len = bitmap_bytes(blocks[&v].iter().filter(|i| specs[**i].optional).count());
        let _ = writeln!(
            ext_block_descs,
            "            ::message_core::ExtBlockLayout {{ version: {v}u16, bitmap_len: {bitmap_len}usize, fixed_len: __L.b{v}_fixed_len }},"
        );
    }
    let mut base_aligns: Vec<String> = vec!["1usize".into()];
    if let Some(idxs) = blocks.get(&1) {
        if bitmap_bytes(idxs.iter().filter(|i| specs[**i].optional).count()) > 0 {
            base_aligns.push("4usize".into());
        }
        for &i in idxs {
            base_aligns.push(align_str(&specs[i].kind));
        }
    }
    let base_max_align = fold_max(&base_aligns, &layout_ty);
    let doc_expr = opt_str(&attrs.doc);
    let ext_versions_arr = format!(
        "&[{}]",
        ext_versions.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")
    );

    let src = format!(
        r#"#[allow(non_snake_case, dead_code)]
pub struct {layout_ty} {{
    {lf_joined}
}}

impl {layout_ty} {{
    const fn __max(a: usize, b: usize) -> usize {{ if a > b {{ a }} else {{ b }} }}
}}

impl {name} {{
    #[allow(non_snake_case)]
    const fn __msg_layout() -> {layout_ty} {{
        {lb_joined}
        {layout_ty} {{ {li_joined} }}
    }}
    #[allow(non_snake_case)]
    const __L: {layout_ty} = Self::__msg_layout();

    pub fn encode(&self) -> ::std::vec::Vec<u8> {{
        const _: () = assert!(<{name} as ::message_core::Fixed>::FIXED_LEN <= ::core::primitive::u16::MAX as usize);
        let mut buf = ::message_core::EncBuf::new();
        let __origin = buf.reserve(<{name} as ::message_core::Fixed>::FIXED_LEN);
        ::message_core::StructEncode::__write_fixed(self, &mut buf, __origin);
        {ext_writes}
        buf.write_u32(0usize, <{name} as ::message_core::Message>::KEY);
        buf.write_u16(4usize, <{name} as ::message_core::Fixed>::FIXED_LEN as u16);
        buf.write_u8(6usize, {max_version}u8);
        buf.write_u8(7usize, 0u8);
        buf.data
    }}

    pub fn decode<'a>(buf: &'a [u8]) -> ::core::option::Option<{reader_ty}<'a>> {{
        if buf.len() < ::message_core::HEADER_LEN + <{name} as ::message_core::Fixed>::FIXED_LEN {{
            return ::core::option::Option::None;
        }}
        if ::message_core::read_u32(buf, 0usize) != <{name} as ::message_core::Message>::KEY {{
            return ::core::option::Option::None;
        }}
        ::core::option::Option::Some({reader_ty} {{
            payload: &buf[::message_core::HEADER_LEN..],
            origin: 0usize,
            end: buf.len() - ::message_core::HEADER_LEN,
            header_version: ::message_core::read_u8(buf, 6usize),
        }})
    }}

    /// Strictly check an untrusted buffer; after `Ok` no accessor on the reader can panic.
    pub fn validate(buf: &[u8]) -> ::core::result::Result<(), ::message_core::ValidationError> {{
        ::message_core::validate_header(buf, <{name} as ::message_core::Message>::KEY, <{name} as ::message_core::Fixed>::FIXED_LEN)?;
        <{name} as ::message_core::StructDecode>::__validate(&buf[::message_core::HEADER_LEN..], 0usize, buf.len() - ::message_core::HEADER_LEN, ::message_core::read_u8(buf, 6usize))
    }}
}}

impl ::message_core::Fixed for {name} {{
    const FIXED_LEN: usize = __L.base_fixed_len;
    const MAX_ALIGN: usize = __L.base_max_align;
    const HAS_EXT: bool = {has_ext};
}}

impl ::message_core::StructEncode for {name} {{
    fn __write_fixed(&self, buf: &mut ::message_core::EncBuf, origin: usize) {{
        {base_writes}
    }}
    fn __write_instance(&self, buf: &mut ::message_core::EncBuf) -> usize {{
        let __start = buf.reserve(<{name} as ::message_core::Fixed>::FIXED_LEN);
        <{name} as ::message_core::StructEncode>::__write_fixed(self, buf, __start);
        {ext_writes}
        __start
    }}
}}

impl ::message_core::StructDecode for {name} {{
    type Reader<'a> = {reader_ty}<'a>;
    fn __reader<'a>(payload: &'a [u8], origin: usize, end: usize, header_version: u8) -> {reader_ty}<'a> {{
        {reader_ty} {{ payload, origin, end, header_version }}
    }}
    fn __owned(reader: &{reader_ty}<'_>) -> {name} {{
        reader.to_owned()
    }}
    fn __validate(payload: &[u8], origin: usize, end: usize, hv: u8) -> ::message_core::VResult {{
        {base_checks}
        {ext_checks}
        ::core::result::Result::Ok(())
    }}
}}

impl ::message_core::Message for {name} {{
    const KEY: u32 = {key}u32;
    const EXT_VERSIONS: &'static [u16] = {ext_versions_arr};
    fn __descriptor() -> &'static ::message_core::MessageDescriptor {{
        static __D: ::std::sync::OnceLock<::message_core::MessageDescriptor> = ::std::sync::OnceLock::new();
        __D.get_or_init(|| ::message_core::MessageDescriptor {{
            key: {key}u32,
            name: ::std::string::String::from("{name}"),
            doc: {doc_expr},
            fields: ::std::vec![
{field_descs}
            ],
            layout: ::message_core::MessageLayout {{
                bitmap_len: __L.base_bitmap_len,
                fixed_len: __L.base_fixed_len,
                max_align: {base_max_align},
                ext_blocks: ::std::vec![
{ext_block_descs}
                ],
            }},
        }})
    }}
}}

::message_core::__inventory::submit! {{
    ::message_core::SchemaEntry::Message(<{name} as ::message_core::Message>::__descriptor)
}}

pub struct {reader_ty}<'a> {{
    payload: &'a [u8],
    origin: usize,
    end: usize,
    header_version: u8,
}}

impl<'a> {reader_ty}<'a> {{
    pub fn payload(&self) -> &'a [u8] {{ self.payload }}
    pub fn header_version(&self) -> u8 {{ self.header_version }}
{accessors}
    pub fn to_owned(&self) -> {name} {{
        {name} {{
{owned_inits}
        }}
    }}
}}

{map_views}
"#,
        lf_joined = lf.join(",\n    "),
        lb_joined = lb.join("\n        "),
        li_joined = li.join(", "),
        has_ext = !ext_versions.is_empty(),
    );

    // Qualify every `__L` reference as `Type::__L` (it is referenced across impls).
    let src = src.replace("__L.", &format!("{name}::__L."));
    if std::env::var("MSG_DUMP").is_ok() {
        let _ = std::fs::write(format!("/tmp/msg-dump-{name}.rs"), &src);
    }
    parse_generated(&src, &name)
}

pub fn expand_enum(input: &syn::DeriveInput) -> Result<proc_macro2::TokenStream, syn::Error> {
    let name = input.ident.to_string();
    let data = match &input.data {
        syn::Data::Enum(e) => e,
        _ => return Err(syn::Error::new_spanned(&input.ident, "MessageEnum only supports enums")),
    };
    let attrs = parse_container_attrs(&input.attrs)?;
    let mut variants: Vec<(String, i32, Option<String>)> = Vec::new();
    let mut next: i32 = 0;
    for v in &data.variants {
        if !matches!(v.fields, syn::Fields::Unit) {
            return Err(syn::Error::new_spanned(&v.ident, "MessageEnum only supports unit variants"));
        }
        let val = match &v.discriminant {
            Some((_, syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(i), .. }))) => {
                i.base10_parse::<i32>()?
            }
            Some(_) => {
                return Err(syn::Error::new_spanned(&v.ident, "enum discriminants must be integer literals"))
            }
            None => next,
        };
        next = val + 1;
        let mut doc_lines: Vec<String> = Vec::new();
        for a in &v.attrs {
            if a.path().is_ident("doc") {
                push_doc_line(a, &mut doc_lines);
            }
        }
        let doc = if doc_lines.is_empty() { None } else { Some(doc_lines.join("\n")) };
        variants.push((v.ident.to_string(), val, doc));
    }

    let arms = variants
        .iter()
        .map(|(n, v, _)| format!("{v} => {name}::{n},"))
        .collect::<Vec<_>>()
        .join(" ");
    let arms_try = variants
        .iter()
        .map(|(n, v, _)| format!("{v} => ::core::option::Option::Some({name}::{n}),"))
        .collect::<Vec<_>>()
        .join(" ");
    let variant_descs = variants
        .iter()
        .map(|(n, v, d)| {
            format!(
                "            ::message_core::EnumVariant {{ name: ::std::string::String::from(\"{n}\"), value: {v}i32, doc: {} }},",
                opt_str(d)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let doc_expr = opt_str(&attrs.doc);

    let src = format!(
        r#"impl ::message_core::Fixed for {name} {{
    const FIXED_LEN: usize = 4;
    const MAX_ALIGN: usize = 4;
    const IS_ENUM: bool = true;
}}

impl ::message_core::StructEncode for {name} {{
    fn __write_fixed(&self, buf: &mut ::message_core::EncBuf, origin: usize) {{
        buf.write_u32(origin, (*self as i32) as u32);
    }}
}}

impl ::message_core::StructDecode for {name} {{
    type Reader<'a> = {name};
    fn __reader<'a>(payload: &'a [u8], origin: usize, _end: usize, _hv: u8) -> {name} {{
        ::message_core::EnumType::__from_i32(::message_core::read_i32(payload, origin))
    }}
    fn __owned(r: &{name}) -> {name} {{ *r }}
    fn __validate(payload: &[u8], origin: usize, _end: usize, _hv: u8) -> ::message_core::VResult {{
        ::message_core::check_enum::<{name}>(payload, origin)
    }}
}}

impl ::message_core::EnumType for {name} {{
    const NAME: &'static str = "{name}";
    fn __to_i32(self) -> i32 {{ self as i32 }}
    fn __from_i32(v: i32) -> Self {{
        match v {{ {arms} _ => ::core::panic!("unknown {name} value: {{v}}") }}
    }}
    fn __try_from_i32(v: i32) -> ::core::option::Option<Self> {{
        match v {{ {arms_try} _ => ::core::option::Option::None }}
    }}
    fn __descriptor() -> &'static ::message_core::EnumDescriptor {{
        static __D: ::std::sync::OnceLock<::message_core::EnumDescriptor> = ::std::sync::OnceLock::new();
        __D.get_or_init(|| ::message_core::EnumDescriptor {{
            name: ::std::string::String::from("{name}"),
            doc: {doc_expr},
            variants: ::std::vec![
{variant_descs}
            ],
        }})
    }}
}}

::message_core::__inventory::submit! {{
    ::message_core::SchemaEntry::Enum(<{name} as ::message_core::EnumType>::__descriptor)
}}
"#,
    );
    parse_generated(&src, &name)
}

// ---------- Attributes ----------

struct MsgAttrs {
    key: Option<u32>,
    doc: Option<String>,
}

fn parse_container_attrs(attrs: &[syn::Attribute]) -> Result<MsgAttrs, syn::Error> {
    let mut out = MsgAttrs { key: None, doc: None };
    let mut doc_lines: Vec<String> = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("msg") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("key") {
                    let v = meta.value()?;
                    let lit: syn::LitInt = v.parse()?;
                    out.key = Some(lit.base10_parse()?);
                    Ok(())
                } else if meta.path.is_ident("doc") {
                    let v = meta.value()?;
                    let lit: syn::LitStr = v.parse()?;
                    out.doc = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("unknown container attribute (supported: key, doc)"))
                }
            })?;
        } else if attr.path().is_ident("doc") {
            push_doc_line(attr, &mut doc_lines);
        }
    }
    if out.doc.is_none() && !doc_lines.is_empty() {
        out.doc = Some(doc_lines.join("\n"));
    }
    Ok(out)
}

fn parse_field_attrs(spec: &mut FieldSpec, attrs: &[syn::Attribute]) -> Result<(), syn::Error> {
    let mut doc_lines: Vec<String> = Vec::new();
    for attr in attrs {
        if attr.path().is_ident("msg") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("since") {
                    let v = meta.value()?;
                    let lit: syn::LitInt = v.parse()?;
                    let since: u16 = lit.base10_parse()?;
                    if !(1..=u8::MAX as u16).contains(&since) {
                        return Err(syn::Error::new_spanned(
                            &lit,
                            format!("since must be in 1..=255 (got {since}); the header version field is a u8"),
                        ));
                    }
                    spec.since = since;
                    Ok(())
                } else if meta.path.is_ident("precise") {
                    spec.precise = true;
                    Ok(())
                } else if meta.path.is_ident("reserved") {
                    spec.reserved = true;
                    Ok(())
                } else if meta.path.is_ident("doc") {
                    let v = meta.value()?;
                    let lit: syn::LitStr = v.parse()?;
                    let d = spec.doc.get_or_insert_with(String::new);
                    d.push_str(&lit.value());
                    Ok(())
                } else {
                    Err(meta.error("unknown field attribute (supported: since, precise, reserved, doc)"))
                }
            })?;
        } else if attr.path().is_ident("doc") {
            push_doc_line(attr, &mut doc_lines);
        }
    }
    if spec.doc.is_none() && !doc_lines.is_empty() {
        spec.doc = Some(doc_lines.join("\n"));
    }
    Ok(())
}

fn push_doc_line(attr: &syn::Attribute, out: &mut Vec<String>) {
    if let syn::Meta::NameValue(nv) = &attr.meta {
        if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
            out.push(s.value().trim_start().to_string());
        }
    }
}

// ---------- Validation ----------

fn check_kind_supported(spec: &FieldSpec) -> Result<(), syn::Error> {
    fn check_elem(elem: &TyKind) -> Result<(), syn::Error> {
        match elem {
            TyKind::Map { .. } => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "maps cannot be array elements or map values in V1",
            )),
            TyKind::Array { fixed: Some(_), .. } => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "fixed arrays cannot be array elements in V1; use Vec<Vec<T>> or split the field",
            )),
            TyKind::Array { elem, .. } => check_elem(elem),
            _ => Ok(()),
        }
    }
    match &spec.kind {
        TyKind::Map { key, value } => {
            match key.as_ref() {
                TyKind::Scalar(_) | TyKind::Str | TyKind::Path(_) => {}
                TyKind::Bytes | TyKind::Array { .. } | TyKind::Map { .. } => {
                    return Err(syn::Error::new(
                        proc_macro2::Span::call_site(),
                        "V1 map keys must be a fixed-size scalar, String, or an enum",
                    ));
                }
            }
            check_elem(value)
        }
        TyKind::Array { elem, .. } => check_elem(elem),
        _ => Ok(()),
    }
}

// ---------- Utilities ----------

fn layout_field(name: &str) -> String {
    format!("f_{name}")
}

fn bitmap_bytes(n: usize) -> usize {
    if n == 0 {
        0
    } else {
        let bytes = (n + 7) / 8;
        (bytes + 3) / 4 * 4
    }
}

fn fold_max(aligns: &[String], layout_ty: &str) -> String {
    let mut acc = "1usize".to_string();
    for a in aligns {
        acc = format!("{layout_ty}::__max({a}, {acc})");
    }
    acc
}

fn opt_str(s: &Option<String>) -> String {
    match s {
        Some(v) => format!("::core::option::Option::Some(::std::string::String::from({v:?}))"),
        None => "::core::option::Option::None".to_string(),
    }
}

fn opt_usize(v: Option<usize>) -> String {
    match v {
        Some(n) => format!("::core::option::Option::Some({n}usize)"),
        None => "::core::option::Option::None".to_string(),
    }
}

fn map_view_ident(field: &str) -> String {
    let pascal: String = field
        .split('_')
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect();
    format!("{pascal}MapView")
}

fn rust_scalar_ty(s: ScalarKind) -> &'static str {
    match s {
        ScalarKind::Bool => "bool",
        ScalarKind::U8 => "u8",
        ScalarKind::I8 => "i8",
        ScalarKind::U16 => "u16",
        ScalarKind::I16 => "i16",
        ScalarKind::U32 => "u32",
        ScalarKind::I32 => "i32",
        ScalarKind::U64 => "u64",
        ScalarKind::I64 => "i64",
        ScalarKind::F32 => "f32",
        ScalarKind::F64 => "f64",
    }
}

fn parse_generated(src: &str, name: &str) -> Result<proc_macro2::TokenStream, syn::Error> {
    src.parse::<proc_macro2::TokenStream>().map_err(|e| {
        syn::Error::new(
            proc_macro2::Span::call_site(),
            format!("failed to parse generated code for `{name}` (internal error): {e}\n----\n{src}"),
        )
    })
}

// ---------- Encode emission ----------

fn sc_write(sc: ScalarKind, pos: &str, val: &str) -> String {
    match sc {
        ScalarKind::Bool | ScalarKind::U8 | ScalarKind::I8 => {
            format!("buf.write_u8({pos}, ({val}) as u8);")
        }
        ScalarKind::U16 | ScalarKind::I16 => format!("buf.write_u16({pos}, ({val}) as u16);"),
        ScalarKind::U32 | ScalarKind::I32 => format!("buf.write_u32({pos}, ({val}) as u32);"),
        ScalarKind::U64 | ScalarKind::I64 => format!("buf.write_u64({pos}, ({val}) as u64);"),
        ScalarKind::F32 => format!("buf.write_u32({pos}, ({val}).to_bits());"),
        ScalarKind::F64 => format!("buf.write_u64({pos}, ({val}).to_bits());"),
    }
}

fn sc_read(sc: ScalarKind, payload: &str, off: &str) -> String {
    match sc {
        ScalarKind::Bool => format!("::message_core::read_bool({payload}, {off})"),
        ScalarKind::U8 => format!("::message_core::read_u8({payload}, {off})"),
        ScalarKind::I8 => format!("::message_core::read_u8({payload}, {off}) as i8"),
        ScalarKind::U16 => format!("::message_core::read_u16({payload}, {off})"),
        ScalarKind::I16 => format!("::message_core::read_i16({payload}, {off})"),
        ScalarKind::U32 => format!("::message_core::read_u32({payload}, {off})"),
        ScalarKind::I32 => format!("::message_core::read_i32({payload}, {off})"),
        ScalarKind::U64 => format!("::message_core::read_u64({payload}, {off})"),
        ScalarKind::I64 => format!("::message_core::read_i64({payload}, {off})"),
        ScalarKind::F32 => format!("::message_core::read_f32({payload}, {off})"),
        ScalarKind::F64 => format!("::message_core::read_f64({payload}, {off})"),
    }
}

fn is_scalar(k: &TyKind) -> bool {
    matches!(k, TyKind::Scalar(_))
}

fn slot_patch(pos: &str, val: &str, len: &str) -> String {
    format!(
        "buf.write_u32({pos}, ({val}) as u32);\nbuf.write_u32({pos} + 4usize, ({len}) as u32);"
    )
}

fn emit_field_write(s: &FieldSpec, opt_bit: Option<usize>, origin: &str, block: &str) -> String {
    let f = &s.name;
    let fdef = layout_field(&s.name);
    let pos = format!("{origin} + __L.{fdef}");
    if s.optional {
        let bit = opt_bit.unwrap_or(0);
        format!(
            "if let ::core::option::Option::Some(__v) = (&self.{f}) {{\n    buf.set_bitmap_bit({block}, {bit}usize);\n    {}\n}}",
            emit_value_write(&s.kind, "(*__v)", &pos, &s.name)
        )
    } else {
        emit_value_write(&s.kind, &format!("self.{}", s.name), &pos, &s.name)
    }
}

fn emit_value_write(kind: &TyKind, val: &str, pos: &str, field: &str) -> String {
    match kind {
        TyKind::Scalar(sc) => sc_write(*sc, pos, val),
        TyKind::Str => format!(
            "{{\n    let __p = buf.append({val}.as_bytes());\n    {}\n}}",
            slot_patch(pos, "__p", &format!("{val}.len()"))
        ),
        TyKind::Bytes => format!(
            "{{\n    let __p = buf.append({val}.as_slice());\n    {}\n}}",
            slot_patch(pos, "__p", &format!("{val}.len()"))
        ),
        TyKind::Path(p) => {
            let t = ty_str(p);
            format!(
                "if <{t} as ::message_core::Fixed>::IS_ENUM {{\n    ::message_core::StructEncode::__write_fixed(&({val}), buf, {pos});\n}} else {{\n    let __s = ::message_core::StructEncode::__write_instance(&({val}), buf);\n    {}\n}}",
                slot_patch(pos, "buf.payload_rel(__s)", "(buf.payload_len() - buf.payload_rel(__s))")
            )
        }
        TyKind::Array { elem, fixed } => emit_array_write(val, elem, *fixed, pos),
        TyKind::Map { .. } => emit_map_write(kind, val, pos, field),
    }
}

/// Element footprint inside an array region plus the shape bit a writer puts into the region
/// slot's length. Struct elements are open (fixed-stride footprints) until the type gains
/// extension fields, after which new writers emit per-element instance slots (shape bit set).
fn elem_stride_and_shape(elem: &TyKind) -> (String, String) {
    match elem {
        TyKind::Path(p) => {
            let t = ty_str(p);
            (
                format!(
                    "(if <{t} as ::message_core::Fixed>::IS_ENUM || !<{t} as ::message_core::Fixed>::HAS_EXT {{ <{t} as ::message_core::Fixed>::FIXED_LEN }} else {{ 8usize }})"
                ),
                format!(
                    "(if <{t} as ::message_core::Fixed>::IS_ENUM || !<{t} as ::message_core::Fixed>::HAS_EXT {{ 0u32 }} else {{ ::message_core::SHAPE_BIT }})"
                ),
            )
        }
        _ => (element_size_str(elem).unwrap_or_else(|_| "8usize".into()), "0u32".to_string()),
    }
}

fn emit_array_write(val: &str, elem: &TyKind, fixed: Option<u32>, pos: &str) -> String {
    let inline = fixed.is_some() && !is_slot_shaped(elem);
    if inline {
        let n = fixed.unwrap();
        match elem {
            TyKind::Scalar(sc) => {
                let w = sc.width();
                format!(
                    "for (i, v) in ({val}).iter().enumerate() {{ {} }}",
                    sc_write(*sc, &format!("{pos} + i * {w}usize"), "*v")
                )
            }
            _ => {
                let stride = element_size_str(elem).unwrap_or_else(|_| "8usize".into());
                format!(
                    "for i in 0..{n}usize {{ ::message_core::StructEncode::__write_fixed(&({val})[i], buf, {pos} + i * {stride}); }}"
                )
            }
        }
    } else {
        let (stride, shape) = elem_stride_and_shape(elem);
        format!(
            "{{\n    let __n = ({val}).len();\n    let __shape: u32 = {shape};\n    let __r0 = buf.reserve(__n * {stride});\n    {}\n    {}\n}}",
            emit_elem_loop(val, elem, "__r0", 0),
            slot_patch(pos, "buf.payload_rel(__r0)", "(__n as u32 | __shape)")
        )
    }
}

fn emit_elem_loop(arr: &str, elem: &TyKind, region: &str, depth: usize) -> String {
    let idx = format!("i{depth}");
    let ev = format!("e{depth}");
    let body = match elem {
        TyKind::Scalar(sc) => {
            let w = sc.width();
            sc_write(*sc, &format!("{region} + {idx} * {w}usize"), &format!("*{ev}"))
        }
        TyKind::Str => format!(
            "{{ let __p = buf.append({ev}.as_bytes()); {} }}",
            slot_patch(&format!("{region} + {idx} * 8usize"), "__p", &format!("{ev}.len()"))
        ),
        TyKind::Bytes => format!(
            "{{ let __p = buf.append({ev}.as_slice()); {} }}",
            slot_patch(&format!("{region} + {idx} * 8usize"), "__p", &format!("{ev}.len()"))
        ),
        TyKind::Path(p) => {
            let t = ty_str(p);
            let inst = slot_patch(
                &format!("{region} + {idx} * 8usize"),
                &format!("buf.payload_rel(__s{depth})"),
                &format!("(buf.payload_len() - buf.payload_rel(__s{depth}))"),
            );
            format!(
                "if <{t} as ::message_core::Fixed>::IS_ENUM || !<{t} as ::message_core::Fixed>::HAS_EXT {{ ::message_core::StructEncode::__write_fixed({ev}, buf, {region} + {idx} * <{t} as ::message_core::Fixed>::FIXED_LEN); }} else {{ let __s{depth} = ::message_core::StructEncode::__write_instance({ev}, buf); {inst} }}"
            )
        }
        TyKind::Array { elem: inner, fixed: _ } => {
            // a nested dynamic array is itself a region slot, so it carries the shape bit too
            let (inner_stride, inner_shape) = elem_stride_and_shape(inner);
            let ir = format!("__r{}", depth + 1);
            let shape_var = format!("__shape{depth}");
            format!(
                "{{ let __m = {ev}.len(); let {shape_var}: u32 = {inner_shape}; let {ir} = buf.reserve(__m * {inner_stride}); {} {} }}",
                emit_elem_loop(&ev.clone(), inner, &ir, depth + 1),
                slot_patch(
                    &format!("{region} + {idx} * 8usize"),
                    &format!("buf.payload_rel({ir})"),
                    &format!("(__m as u32 | {shape_var})")
                )
            )
        }
        TyKind::Map { .. } => String::new(),
    };
    format!("for ({idx}, {ev}) in ({arr}).iter().enumerate() {{ {body} }}")
}

fn emit_map_write(kind: &TyKind, val: &str, pos: &str, field: &str) -> String {
    let (key, value) = match kind {
        TyKind::Map { key, value } => (key, value),
        _ => return String::new(),
    };
    // The entry layout depends on the value's shape: an evolved struct value is written as an
    // instance slot (stride1/v_off1 + shape bit), everything else uses the open pair.
    let (prelude, v_shape_branch) = match value.as_ref() {
        TyKind::Path(p) => {
            let t = ty_str(p);
            (
                format!(
                    "let __has_ext = !<{t} as ::message_core::Fixed>::IS_ENUM && <{t} as ::message_core::Fixed>::HAS_EXT;\n    let __stride = if __has_ext {{ __L.m_{field}_stride1 }} else {{ __L.m_{field}_stride0 }};\n    let __v_off = if __has_ext {{ __L.m_{field}_v_off1 }} else {{ __L.m_{field}_v_off0 }};\n    let __shape: u32 = if __has_ext {{ ::message_core::SHAPE_BIT }} else {{ 0u32 }};"
                ),
                true,
            )
        }
        _ => (
            format!(
                "let __stride = __L.m_{field}_stride0;\n    let __v_off = __L.m_{field}_v_off0;\n    let __shape: u32 = 0u32;"
            ),
            false,
        ),
    };
    let k_pos = "__e".to_string();
    let v_pos = "__e + __v_off".to_string();
    let (k_write, sort) = match key.as_ref() {
        TyKind::Scalar(sc) => (sc_write(*sc, &k_pos, "(**k)"), "__entries.sort_by(|a, b| a.0.cmp(b.0));".to_string()),
        TyKind::Str => (
            format!(
                "{{ let __p = buf.append((**k).as_bytes()); {} }}",
                slot_patch(&k_pos, "__p", "(**k).len()")
            ),
            "__entries.sort_by(|a, b| a.0.cmp(b.0));".to_string(),
        ),
        TyKind::Path(_) => (
            format!("buf.write_u32({k_pos}, ::message_core::EnumType::__to_i32(**k) as u32);"),
            "__entries.sort_by_key(|(k, _)| ::message_core::EnumType::__to_i32(**k));".to_string(),
        ),
        _ => return String::new(),
    };
    let v_write = match value.as_ref() {
        TyKind::Scalar(sc) => sc_write(*sc, &v_pos, "(**v)"),
        TyKind::Str => format!(
            "{{ let __p = buf.append((**v).as_bytes()); {} }}",
            slot_patch(&v_pos, "__p", "(**v).len()")
        ),
        TyKind::Bytes => format!(
            "{{ let __p = buf.append((**v).as_slice()); {} }}",
            slot_patch(&v_pos, "__p", "(**v).len()")
        ),
        TyKind::Path(_) => {
            if v_shape_branch {
                let inst = slot_patch(
                    &v_pos,
                    "buf.payload_rel(__vs)",
                    "(buf.payload_len() - buf.payload_rel(__vs))",
                );
                format!(
                    "if !__has_ext {{ ::message_core::StructEncode::__write_fixed(*v, buf, {v_pos}); }} else {{ let __vs = ::message_core::StructEncode::__write_instance(*v, buf); {inst} }}"
                )
            } else {
                format!("::message_core::StructEncode::__write_fixed(*v, buf, {v_pos});")
            }
        }
        TyKind::Array { elem, fixed } => emit_array_write("(**v)", elem, *fixed, &v_pos),
        _ => String::new(),
    };
    format!(
        "{{\n    let mut __entries: ::std::vec::Vec<_> = ({val}).iter().collect();\n    {sort}\n    {prelude}\n    let __n = __entries.len();\n    let __region = buf.reserve(__n * __stride);\n    for (i, (k, v)) in __entries.iter().enumerate() {{\n        let __e = __region + i * __stride;\n        {k_write}\n        {v_write}\n    }}\n    {}\n}}",
        slot_patch(pos, "buf.payload_rel(__region)", "(__n as u32 | __shape)")
    )
}

// ---------- Accessor emission ----------

fn view_ret_ty(kind: &TyKind) -> String {
    match kind {
        TyKind::Scalar(sc) => rust_scalar_ty(*sc).to_string(),
        TyKind::Str => "&'a str".into(),
        TyKind::Bytes => "&'a [u8]".into(),
        TyKind::Path(p) => format!("<{} as ::message_core::StructDecode>::Reader<'a>", ty_str(p)),
        TyKind::Array { elem, .. } => match elem.as_ref() {
            TyKind::Scalar(sc) => {
                format!("::message_core::ScalarListView<'a, {}>", rust_scalar_ty(*sc))
            }
            TyKind::Str => "::message_core::StringListView<'a>".into(),
            TyKind::Bytes => "::message_core::BytesListView<'a>".into(),
            TyKind::Path(p) => {
                format!("::message_core::StructEvoListView<'a, {}>", ty_str(p))
            }
            _ => "::message_core::SlotListView<'a>".into(),
        },
        TyKind::Map { .. } => String::new(),
    }
}

fn view_ctor(kind: &TyKind, payload: &str, start: &str, count: &str) -> String {
    match kind {
        TyKind::Str => format!("::message_core::StringListView::new({payload}, {start}, {count})"),
        TyKind::Bytes => format!("::message_core::BytesListView::new({payload}, {start}, {count})"),
        TyKind::Array { elem, .. } => match elem.as_ref() {
            TyKind::Scalar(_) => {
                format!("::message_core::ScalarListView::new({payload}, {start}, {count})")
            }
            TyKind::Str => {
                format!("::message_core::StringListView::new({payload}, {start}, {count})")
            }
            TyKind::Bytes => {
                format!("::message_core::BytesListView::new({payload}, {start}, {count})")
            }
            TyKind::Path(p) => format!(
                "::message_core::StructEvoListView::<{}>::new({payload}, {start}, {count} as u32)",
                ty_str(p)
            ),
            _ => format!("::message_core::SlotListView::new({payload}, {start}, {count})"),
        },
        _ => String::new(),
    }
}

fn default_expr(kind: &TyKind, field: &str) -> String {
    match kind {
        TyKind::Scalar(_) => "::core::default::Default::default()".into(),
        TyKind::Path(_) => {
            format!("::core::panic!(\"field `{field}` missing: data written by an older schema version\")")
        }
        TyKind::Str => "\"\"".into(),
        TyKind::Bytes => "&[]".into(),
        TyKind::Array { .. } => view_ctor(kind, "self.payload", "0usize", "0usize"),
        TyKind::Map { .. } => format!(
            "{} {{ payload: self.payload, start: 0usize, raw_len: 0u32 }}",
            map_view_ident(field)
        ),
    }
}

fn emit_accessor(s: &FieldSpec, parent: &str, opt_bit: Option<usize>) -> String {
    let f = &s.name;
    let fdef = layout_field(f);
    let ext = s.since > 1;
    let map_view = if matches!(s.kind, TyKind::Map { .. }) {
        Some(map_view_ident(f))
    } else {
        None
    };
    let ret = match &map_view {
        Some(v) => format!("{v}<'a>"),
        None => view_ret_ty(&s.kind),
    };

    // Field read expressions (origin is the block start).
    let body = |origin: &str| -> String {
        let pos = format!("{origin} + __L.{fdef}");
        match &s.kind {
            TyKind::Scalar(sc) => sc_read(*sc, "self.payload", &pos),
            TyKind::Str => format!(
                "{{ let (o, l) = ::message_core::read_slot(self.payload, {pos}); ::core::str::from_utf8(::message_core::slot_slice(self.payload, o, l)).expect(\"invalid utf-8 in field `{f}`\") }}"
            ),
            TyKind::Bytes => format!(
                "{{ let (o, l) = ::message_core::read_slot(self.payload, {pos}); ::message_core::slot_slice(self.payload, o, l) }}"
            ),
            TyKind::Path(p) => {
                let t = ty_str(p);
                format!(
                    "if <{t} as ::message_core::Fixed>::IS_ENUM {{ <{t} as ::message_core::StructDecode>::__reader(self.payload, {pos}, self.payload.len(), 0u8) }} else {{ let (o, l) = ::message_core::read_slot(self.payload, {pos}); <{t} as ::message_core::StructDecode>::__reader(self.payload, o as usize, o as usize + l as usize, ::message_core::INSTANCE_HV) }}"
                )
            }
            TyKind::Array { elem, fixed } => {
                if fixed.is_some() && !is_slot_shaped(elem) {
                    let n = fixed.unwrap();
                    view_ctor(&s.kind, "self.payload", &pos, &format!("{n}usize"))
                } else {
                    format!(
                        "{{ let (o, l) = ::message_core::read_slot(self.payload, {pos}); {} }}",
                        view_ctor(&s.kind, "self.payload", "o as usize", "l as usize")
                    )
                }
            }
            TyKind::Map { .. } => format!(
                "{{ let (o, l) = ::message_core::read_slot(self.payload, {pos}); {} {{ payload: self.payload, start: o as usize, raw_len: l }} }}",
                map_view.clone().unwrap()
            ),
        }
    };

    let bitmap_check = |origin: &str| -> String {
        let bit = opt_bit.unwrap_or(0);
        format!("if !::message_core::read_bitmap_bit(&self.payload[{origin}..], {bit}usize) {{ return ::core::option::Option::None; }}")
    };

    if ext {
        let find = format!(
            "::message_core::find_ext_block_bounded(self.payload, <{parent} as ::message_core::Message>::EXT_VERSIONS, self.header_version, {since}u16, __L.b{since}_fixed_len, self.origin, self.end)",
            since = s.since
        );
        if s.optional {
            format!(
                "    pub fn {f}(&self) -> ::core::option::Option<{ret}> {{\n        let (__bs, _) = {find}?;\n        {}\n        ::core::option::Option::Some({})\n    }}",
                bitmap_check("__bs"),
                body("__bs")
            )
        } else {
            format!(
                "    pub fn {f}(&self) -> {ret} {{\n        match {find} {{\n            ::core::option::Option::Some((__bs, _)) => {{ {} }},\n            ::core::option::Option::None => {},\n        }}\n    }}",
                body("__bs"),
                default_expr(&s.kind, f)
            )
        }
    } else if s.optional {
        format!(
            "    pub fn {f}(&self) -> ::core::option::Option<{ret}> {{\n        {}\n        ::core::option::Option::Some({})\n    }}",
            bitmap_check("self.origin"),
            body("self.origin")
        )
    } else {
        format!(
            "    pub fn {f}(&self) -> {ret} {{\n        {}\n    }}",
            body("self.origin")
        )
    }
}

// ---------- Validate emission ----------

/// Strict check of one field located at `origin + __L.f_<name>` (optional fields only when their
/// bitmap bit is set).
fn emit_field_validate(s: &FieldSpec, opt_bit: Option<usize>, origin: &str) -> String {
    let pos = format!("{origin} + __L.{}", layout_field(&s.name));
    let check = emit_validate_value(&s.kind, &pos, &s.name, "::core::option::Option::None", "hv", 0);
    if s.optional {
        let bit = opt_bit.unwrap_or(0);
        format!("if ::message_core::read_bitmap_bit(&payload[{origin}..], {bit}usize) {{ {check} }}")
    } else {
        check
    }
}

/// Statement(s) strictly checking a value of `kind` at absolute payload position `pos`. Leaf
/// errors are nested under `field` (element `idx`); `depth` keeps loop variables unique.
fn emit_validate_value(kind: &TyKind, pos: &str, field: &str, idx: &str, hv: &str, depth: usize) -> String {
    let nest = |index: &str| format!(".map_err(|e| ::message_core::ValidationError::nested(e, \"{field}\", {index}))?");
    let call = |expr: String, index: &str| format!("{expr}{};", nest(index));
    match kind {
        TyKind::Scalar(_) => String::new(),
        TyKind::Str => call(format!("::message_core::check_str_slot(payload, {pos})"), idx),
        TyKind::Bytes => call(format!("::message_core::check_bytes_slot(payload, {pos})"), idx),
        TyKind::Path(p) => {
            let t = ty_str(p);
            // enums are inline i32s; structs are self-contained instances referenced by a slot
            let inst = format!(
                "{{ let (__lo{depth}, __hi{depth}) = ::message_core::check_instance_slot(payload, {pos}){}; {} }}",
                nest(idx),
                call(
                    format!(
                        "<{t} as ::message_core::StructDecode>::__validate(payload, __lo{depth}, __hi{depth}, ::message_core::INSTANCE_HV)"
                    ),
                    idx
                )
            );
            format!(
                "if <{t} as ::message_core::Fixed>::IS_ENUM {{ {} }} else {{ {inst} }}",
                call(
                    format!("<{t} as ::message_core::StructDecode>::__validate(payload, {pos}, payload.len(), {hv})"),
                    idx
                )
            )
        }
        TyKind::Array { elem, fixed } => {
            let stride = element_size_str(elem).unwrap_or_else(|_| "8usize".into());
            let i = format!("i{depth}");
            let idx_some = format!("::core::option::Option::Some({i})");
            if fixed.is_some() && !is_slot_shaped(elem) {
                let body = emit_validate_value(elem, &format!("{pos} + {i} * {stride}"), field, &idx_some, "0u8", depth + 1);
                if body.is_empty() {
                    String::new()
                } else {
                    format!("for {i} in 0..{}usize {{ {body} }}", fixed.unwrap())
                }
            } else if let TyKind::Path(p) = elem.as_ref() {
                // struct elements: the region slot's shape bit selects open footprints vs slots
                let t = ty_str(p);
                let (o, n) = (format!("__o{depth}"), format!("__n{depth}"));
                let (lo, hi) = (format!("__il{depth}"), format!("__ih{depth}"));
                format!(
                    "let ({o}, {n}, __sp{depth}) = ::message_core::read_region_slot(payload, {pos}){}; if __sp{depth} {{ for {i} in 0..{n} {{ let ({lo}, {hi}) = ::message_core::check_instance_slot(payload, {o} + {i} * 8usize){}; {} }} }} else {{ {} ; for {i} in 0..{n} {{ {} }} }}",
                    nest(idx),
                    nest(&idx_some),
                    call(
                        format!("<{t} as ::message_core::StructDecode>::__validate(payload, {lo}, {hi}, ::message_core::INSTANCE_HV)"),
                        &idx_some
                    ),
                    call(format!("::message_core::check_extent(payload, {o}, {n}, <{t} as ::message_core::Fixed>::FIXED_LEN)"), idx),
                    call(
                        format!("<{t} as ::message_core::StructDecode>::__validate(payload, {o} + {i} * <{t} as ::message_core::Fixed>::FIXED_LEN, payload.len(), 0u8)"),
                        &idx_some
                    ),
                )
            } else {
                let (o, n, p) = (format!("__o{depth}"), format!("__n{depth}"), format!("__p{depth}"));
                let body = emit_validate_value(elem, &p, field, &idx_some, "0u8", depth + 1);
                if body.is_empty() {
                    call(format!("::message_core::check_region(payload, {pos}, {stride})"), idx)
                } else {
                    format!(
                        "let ({o}, {n}) = ::message_core::check_region(payload, {pos}, {stride}){}; for {i} in 0..{n} {{ let {p} = {o} + {i} * {stride}; {body} }}",
                        nest(idx)
                    )
                }
            }
        }
        TyKind::Map { key, value } => {
            let (o, n, i, e, vo) = (
                format!("__o{depth}"),
                format!("__n{depth}"),
                format!("i{depth}"),
                format!("__e{depth}"),
                format!("__vo{depth}"),
            );
            let v_pos = format!("{e} + {vo}");
            let idx_some = format!("::core::option::Option::Some({i})");
            if is_scalar(key) && is_scalar(value) {
                // pure scalar maps: no shape bit is ever written, the open stride is final
                return format!(
                    "::message_core::check_region(payload, {pos}, __L.m_{field}_stride0){};",
                    nest(idx)
                );
            }
            let k = emit_validate_value(key, &e, field, &idx_some, "0u8", depth + 1);
            let v = emit_validate_value(value, &v_pos, field, &idx_some, "0u8", depth + 1);
            format!(
                "let ({o}, {n}, __sm{depth}) = ::message_core::read_region_slot(payload, {pos}){}; let __st{depth} = if __sm{depth} {{ __L.m_{field}_stride1 }} else {{ __L.m_{field}_stride0 }}; let {vo} = if __sm{depth} {{ __L.m_{field}_v_off1 }} else {{ __L.m_{field}_v_off0 }}; {} ; for {i} in 0..{n} {{ let {e} = {o} + {i} * __st{depth}; {k} {v} }}",
                nest(idx),
                call(format!("::message_core::check_extent(payload, {o}, {n}, __st{depth})"), idx),
            )
        }
    }
}

// ---------- Map views ----------

fn emit_map_view(s: &FieldSpec, key: &TyKind, value: &TyKind, _parent: &str) -> String {
    let f = &s.name;
    let view = map_view_ident(f);

    let key_ret = match key {
        TyKind::Scalar(sc) => rust_scalar_ty(*sc).to_string(),
        TyKind::Path(p) => ty_str(p),
        _ => "&'a str".to_string(),
    };
    let val_ret = match value {
        TyKind::Scalar(sc) => rust_scalar_ty(*sc).to_string(),
        TyKind::Str => "&'a str".to_string(),
        TyKind::Bytes => "&'a [u8]".to_string(),
        TyKind::Path(p) => {
            format!("<{} as ::message_core::StructDecode>::Reader<'a>", ty_str(p))
        }
        TyKind::Array { .. } => view_ret_ty(value),
        _ => "::core::unreachable!()".to_string(),
    };
    let key_read = |pos: &str| -> String {
        match key {
            TyKind::Scalar(sc) => sc_read(*sc, "self.payload", pos),
            TyKind::Path(_) => format!(
                "::message_core::EnumType::__from_i32(::message_core::read_i32(self.payload, {pos}))"
            ),
            _ => format!(
                "{{ let (o, l) = ::message_core::read_slot(self.payload, {pos}); ::core::str::from_utf8(::message_core::slot_slice(self.payload, o, l)).expect(\"invalid utf-8 in map key\") }}"
            ),
        }
    };
    let val_read = |pos: &str| -> String {
        match value {
            TyKind::Scalar(sc) => sc_read(*sc, "self.payload", pos),
            TyKind::Str => format!(
                "{{ let (o, l) = ::message_core::read_slot(self.payload, {pos}); ::core::str::from_utf8(::message_core::slot_slice(self.payload, o, l)).expect(\"invalid utf-8 in map value\") }}"
            ),
            TyKind::Bytes => format!(
                "{{ let (o, l) = ::message_core::read_slot(self.payload, {pos}); ::message_core::slot_slice(self.payload, o, l) }}"
            ),
            TyKind::Path(p) => {
                let t = ty_str(p);
                // Enum values are inline i32s (their __reader decodes them directly); struct
                // values are open elements (shape=0) or instance slots (shape=1) depending on
                // the writer's schema version.
                format!(
                    "if <{t} as ::message_core::Fixed>::IS_ENUM {{ <{t} as ::message_core::StructDecode>::__reader(self.payload, {pos}, self.payload.len(), 0u8) }} else if self.is_inst() {{ let (o, l) = ::message_core::read_slot(self.payload, {pos}); <{t} as ::message_core::StructDecode>::__reader(self.payload, o as usize, o as usize + l as usize, ::message_core::INSTANCE_HV) }} else {{ <{t} as ::message_core::StructDecode>::__reader(self.payload, {pos}, self.payload.len(), 0u8) }}"
                )
            }
            TyKind::Array { .. } => format!(
                "{{ let (o, l) = ::message_core::read_slot(self.payload, {pos}); {} }}",
                view_ctor(value, "self.payload", "o as usize", "l as usize")
            ),
            _ => "::core::unreachable!()".to_string(),
        }
    };

    format!(
        r#"#[allow(non_snake_case)]
pub struct {view}<'a> {{
    payload: &'a [u8],
    start: usize,
    raw_len: u32,
}}

impl<'a> {view}<'a> {{
    pub fn len(&self) -> usize {{ (self.raw_len & ::message_core::LEN_MASK) as usize }}
    pub fn is_empty(&self) -> bool {{ self.len() == 0 }}
    fn is_inst(&self) -> bool {{ self.raw_len & ::message_core::SHAPE_BIT != 0 }}
    fn stride(&self) -> usize {{ if self.is_inst() {{ __L.m_{f}_stride1 }} else {{ __L.m_{f}_stride0 }} }}
    fn v_off(&self) -> usize {{ if self.is_inst() {{ __L.m_{f}_v_off1 }} else {{ __L.m_{f}_v_off0 }} }}
    pub fn key(&self, i: usize) -> {key_ret} {{
        ::core::assert!(i < self.len());
        let __e = self.start + i * self.stride();
        {kr}
    }}
    pub fn value(&self, i: usize) -> {val_ret} {{
        ::core::assert!(i < self.len());
        let __e = self.start + i * self.stride() + self.v_off();
        {vr}
    }}
    pub fn entry(&self, i: usize) -> ({key_ret}, {val_ret}) {{ (self.key(i), self.value(i)) }}
}}
"#,
        kr = key_read("__e"),
        vr = val_read("__e"),
    )
}

// ---------- to_owned ----------

fn owned_field_expr(s: &FieldSpec) -> String {
    if s.reserved {
        return "::core::default::Default::default()".into();
    }
    let base = format!("self.{}()", s.name);
    if s.optional {
        format!(
            "{}.map(|v| {})",
            base,
            owned_inner_expr(&s.kind, "v", &s.ty)
        )
    } else {
        owned_inner_expr(&s.kind, &base, &s.ty)
    }
}

fn owned_inner_expr(kind: &TyKind, base: &str, ty: &syn::Type) -> String {
    let ty_s = ty_str(ty);
    match kind {
        TyKind::Scalar(_) => base.to_string(),
        TyKind::Str => format!("{base}.to_string()"),
        TyKind::Bytes => format!("{base}.to_vec()"),
        TyKind::Path(p) => format!(
            "<{} as ::message_core::StructDecode>::__owned(&{base})",
            ty_str(p)
        ),
        TyKind::Array { elem, fixed } => {
            let inner = owned_list_expr(base, elem, "self.payload");
            if fixed.is_some() {
                format!(
                    "{{ let r: ::std::vec::Vec<_> = {inner}; let r: {ty_s} = r.try_into().unwrap_or_else(|_| ::core::unreachable!()); r }}"
                )
            } else {
                inner
            }
        }
        TyKind::Map { key, value } => {
            let k_owned = match key.as_ref() {
                TyKind::Str => "k.to_string()".to_string(),
                _ => "k".to_string(),
            };
            let v_owned = match value.as_ref() {
                TyKind::Str => "w.to_string()".to_string(),
                TyKind::Bytes => "w.to_vec()".to_string(),
                TyKind::Path(p) => {
                    format!("<{} as ::message_core::StructDecode>::__owned(&w)", ty_str(p))
                }
                TyKind::Array { elem, .. } => owned_list_expr("w", elem, "self.payload"),
                _ => "w".to_string(),
            };
            format!(
                "{{ let v = {base}; (0..v.len()).map(|i| {{ let (k, w) = v.entry(i); ({k_owned}, {v_owned}) }}).collect::<{ty_s}>() }}"
            )
        }
    }
}

fn owned_list_expr(view: &str, elem: &TyKind, payload: &str) -> String {
    match elem {
        TyKind::Scalar(_) => format!("(0..{view}.len()).map(|i| {view}.get(i)).collect::<::std::vec::Vec<_>>()"),
        TyKind::Str => format!(
            "(0..{view}.len()).map(|i| {view}.get(i).to_string()).collect::<::std::vec::Vec<_>>()"
        ),
        TyKind::Bytes => format!(
            "(0..{view}.len()).map(|i| {view}.get(i).to_vec()).collect::<::std::vec::Vec<_>>()"
        ),
        TyKind::Path(p) => format!(
            "(0..{view}.len()).map(|i| <{} as ::message_core::StructDecode>::__owned(&{view}.get(i))).collect::<::std::vec::Vec<_>>()",
            ty_str(p)
        ),
        TyKind::Array { elem: inner, .. } => {
            let inner_ctor = view_ctor(elem, payload, "o as usize", "l as usize");
            let inner_owned = owned_list_expr("iv", inner, payload);
            format!(
                "(0..{view}.len()).map(|i| {{ let (o, l) = {view}.slot_of(i); let iv = {inner_ctor}; {inner_owned} }}).collect::<::std::vec::Vec<_>>()"
            )
        }
        TyKind::Map { .. } => String::new(),
    }
}

// ---------- Descriptor ----------

fn field_desc_body(s: &FieldSpec, opt_bit: Option<usize>) -> String {
    let slot_b = match &s.kind {
        // a referenced enum is an inline i32, a referenced struct is a slot
        TyKind::Path(p) => format!(
            "if <{} as ::message_core::Fixed>::IS_ENUM {{ false }} else {{ true }}",
            ty_str(p)
        ),
        _ => format!("{}", is_slot_shaped(&s.kind)),
    };
    let is_path_valued_map = matches!(&s.kind, TyKind::Map { value, .. } if matches!(value.as_ref(), TyKind::Path(_)));
    let elem_size = match &s.kind {
        TyKind::Array { elem, .. } => {
            format!(
                "::core::option::Option::Some({})",
                element_size_str(elem).unwrap_or_else(|_| "8usize".into())
            )
        }
        TyKind::Map { value, .. } => {
            if is_path_valued_map {
                let t = match value.as_ref() {
                    TyKind::Path(p) => ty_str(p),
                    _ => unreachable!(),
                };
                format!(
                    "::core::option::Option::Some(if <{t} as ::message_core::Fixed>::HAS_EXT {{ __L.m_{}_stride1 }} else {{ __L.m_{}_stride0 }})",
                    s.name, s.name
                )
            } else {
                format!("::core::option::Option::Some(__L.m_{}_stride0)", s.name)
            }
        }
        _ => "::core::option::Option::None".to_string(),
    };
    let map_vo = match &s.kind {
        TyKind::Map { value, .. } => {
            if is_path_valued_map {
                let t = match value.as_ref() {
                    TyKind::Path(p) => ty_str(p),
                    _ => unreachable!(),
                };
                format!(
                    "::core::option::Option::Some(if <{t} as ::message_core::Fixed>::HAS_EXT {{ __L.m_{}_v_off1 }} else {{ __L.m_{}_v_off0 }})",
                    s.name, s.name
                )
            } else {
                format!("::core::option::Option::Some(__L.m_{}_v_off0)", s.name)
            }
        }
        _ => "::core::option::Option::None".to_string(),
    };
    format!(
        "name: ::std::string::String::from(\"{n}\"), ty: {ty}, optional: {opt}, since: {since}, doc: {doc}, precise: {precise}, reserved: {res}, offset: __L.{fdef}, size: {size}, optional_bit: {ob}, slot: {slot}, element_size: {es}, map_value_offset: {mvo}",
        n = s.name,
        ty = type_expr_str(&s.kind),
        opt = s.optional,
        since = s.since,
        doc = opt_str(&s.doc),
        precise = s.precise,
        res = s.reserved,
        fdef = layout_field(&s.name),
        size = size_str(&s.kind),
        ob = opt_usize(opt_bit),
        slot = slot_b,
        es = elem_size,
        mvo = map_vo,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(src: &str) -> Result<proc_macro2::TokenStream, syn::Error> {
        let input: syn::DeriveInput = syn::parse_str(src).unwrap();
        expand_struct(&input)
    }

    #[test]
    fn since_outside_1_to_255_is_a_compile_error() {
        for bad in ["#[msg(since = 300)]", "#[msg(since = 0)]"] {
            let err = expand(&format!("#[msg(key = 1)] struct X {{ {bad} a: u8 }}")).unwrap_err();
            assert!(err.to_string().contains("1..=255"), "{bad}: {err}");
        }
        assert!(expand("#[msg(key = 1)] struct X { #[msg(since = 255)] a: u8 }").is_ok());
    }

    #[test]
    fn map_key_rules_match_the_core_layer() {
        for bad in ["Vec<u8>", "Vec<u32>", "std::collections::BTreeMap<String, u32>"] {
            let err = expand(&format!(
                "#[msg(key = 1)] struct X {{ m: std::collections::HashMap<{bad}, u32> }}"
            ))
            .unwrap_err();
            assert!(err.to_string().contains("map keys"), "{bad}: {err}");
        }
        let ok = [
            "std::collections::HashMap<Color, u32>",
            "std::collections::HashMap<String, Vec<String>>",
            "std::collections::HashMap<u64, Vec<u8>>",
        ];
        for good in ok {
            assert!(expand(&format!("#[msg(key = 1)] struct X {{ m: {good} }}")).is_ok(), "{good}");
        }
        assert!(
            expand("#[msg(key = 1)] struct X { m: std::collections::HashMap<String, [u8; 4]> }")
                .is_err(),
            "fixed arrays cannot be map values"
        );
    }
}
