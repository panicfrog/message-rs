//! Runtime base: encoding buffer, fixed-size little-endian read primitives, extension-block
//! back-walk locator, and lazy views.
//!
//! Slot semantics: `{ offset: u32, len: u32 }` where offset is relative to the payload start
//! (i.e. after the first HEADER_LEN bytes of the full buffer); len is bytes for string/bytes and
//! element count for arrays. Extension blocks are laid out as
//! `[ bitmap | fixed fields | var data | since:u16 | block_len:u32 ]`, where the trailer's
//! block_len covers the whole block (trailer included).

use crate::{HEADER_LEN, SLOT_LEN};

/// Encoding buffer. Owns the full buffer (including the 8-byte header) and handles slot
/// placeholder/patching and payload-relative offset conversion.
pub struct EncBuf {
    pub data: Vec<u8>,
    payload_origin: usize,
}

impl EncBuf {
    /// Create an empty buffer with the header reserved.
    pub fn new() -> Self {
        EncBuf { data: vec![0u8; HEADER_LEN], payload_origin: HEADER_LEN }
    }

    pub fn payload_origin(&self) -> usize {
        self.payload_origin
    }

    /// Current payload length.
    pub fn payload_len(&self) -> usize {
        self.data.len() - self.payload_origin
    }

    /// Append bytes and return the payload-relative offset.
    pub fn append(&mut self, bytes: &[u8]) -> usize {
        let off = self.payload_len();
        self.data.extend_from_slice(bytes);
        off
    }

    /// Reserve n zeroed bytes (contiguous fixed region / array element region), returning the absolute start.
    pub fn reserve(&mut self, n: usize) -> usize {
        let start = self.data.len();
        self.data.resize(start + n, 0);
        start
    }

    /// Reserve a reference slot, returning its absolute position (patched later via two write_u32 calls).
    pub fn placeholder_slot(&mut self) -> usize {
        self.reserve(SLOT_LEN)
    }

    pub fn patch_slot(&mut self, pos: usize, offset: u32, len: u32) {
        self.write_u32(pos, offset);
        self.write_u32(pos + 4, len);
    }

    pub fn payload_rel(&self, abs: usize) -> usize {
        abs - self.payload_origin
    }

    /// Set a bit in a block's bitmap (block start is absolute; the bitmap leads the block).
    pub fn set_bitmap_bit(&mut self, block_start: usize, bit: usize) {
        self.data[block_start + bit / 8] |= 1 << (bit % 8);
    }

    pub fn write_u8(&mut self, pos: usize, v: u8) {
        self.data[pos] = v;
    }
    pub fn write_u16(&mut self, pos: usize, v: u16) {
        self.data[pos..pos + 2].copy_from_slice(&v.to_le_bytes());
    }
    pub fn write_u32(&mut self, pos: usize, v: u32) {
        self.data[pos..pos + 4].copy_from_slice(&v.to_le_bytes());
    }
    pub fn write_u64(&mut self, pos: usize, v: u64) {
        self.data[pos..pos + 8].copy_from_slice(&v.to_le_bytes());
    }
}

impl Default for EncBuf {
    fn default() -> Self {
        Self::new()
    }
}

// ---- Read primitives (bounds-checked; out-of-bounds panics name the offset) ----

fn check(buf: &[u8], off: usize, n: usize) {
    assert!(off + n <= buf.len(), "message read out of bounds: off={off} n={n} len={}", buf.len());
}

pub fn read_u8(buf: &[u8], off: usize) -> u8 {
    check(buf, off, 1);
    buf[off]
}
pub fn read_bool(buf: &[u8], off: usize) -> bool {
    check(buf, off, 1);
    buf[off] != 0
}
pub fn read_u16(buf: &[u8], off: usize) -> u16 {
    check(buf, off, 2);
    u16::from_le_bytes(buf[off..off + 2].try_into().unwrap())
}
pub fn read_i16(buf: &[u8], off: usize) -> i16 {
    check(buf, off, 2);
    i16::from_le_bytes(buf[off..off + 2].try_into().unwrap())
}
pub fn read_u32(buf: &[u8], off: usize) -> u32 {
    check(buf, off, 4);
    u32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}
pub fn read_i32(buf: &[u8], off: usize) -> i32 {
    check(buf, off, 4);
    i32::from_le_bytes(buf[off..off + 4].try_into().unwrap())
}
pub fn read_u64(buf: &[u8], off: usize) -> u64 {
    check(buf, off, 8);
    u64::from_le_bytes(buf[off..off + 8].try_into().unwrap())
}
pub fn read_i64(buf: &[u8], off: usize) -> i64 {
    check(buf, off, 8);
    i64::from_le_bytes(buf[off..off + 8].try_into().unwrap())
}
pub fn read_f32(buf: &[u8], off: usize) -> f32 {
    f32::from_bits(read_u32(buf, off))
}
pub fn read_f64(buf: &[u8], off: usize) -> f64 {
    f64::from_bits(read_u64(buf, off))
}

/// Read a reference slot, returning (payload-relative offset, len).
pub fn read_slot(buf: &[u8], off: usize) -> (u32, u32) {
    (read_u32(buf, off), read_u32(buf, off + 4))
}

/// Read a bitmap bit.
pub fn read_bitmap_bit(buf: &[u8], bit: usize) -> bool {
    let byte = read_u8(buf, bit / 8);
    (byte >> (bit % 8)) & 1 == 1
}

/// Slice by payload-relative slot value (bounds-checked).
pub fn slot_slice<'a>(payload: &'a [u8], off: u32, len: u32) -> &'a [u8] {
    let off = off as usize;
    let len = len as usize;
    assert!(off + len <= payload.len(), "slot out of bounds: off={off} len={len} payload={}", payload.len());
    &payload[off..off + len]
}

/// Locate the extension block of the target version by walking back from the payload end,
/// returning its payload-relative range `[start, end)` (trailer excluded). `versions` lists all
/// extension versions this reader knows (ascending); `header_version` is the writer's maximum;
/// `min_len` is the block's fixed region (bitmap + fixed fields), so a block too short to hold
/// its fields is treated as absent. Blocks newer than the target are stepped over even when this
/// reader does not know them (an older reader must still find its own blocks behind a newer
/// writer's). Corrupt data yields None (accessors degrade to defaults; strict checking goes
/// through validate).
pub fn find_ext_block(payload: &[u8], versions: &[u16], header_version: u8, target: u16, min_len: usize) -> Option<(usize, usize)> {
    let max_known = versions.last().copied().unwrap_or(0);
    let mut pos = payload.len();
    loop {
        if pos < crate::EXT_TRAILER_LEN {
            return None;
        }
        let since = read_u16(payload, pos - 6);
        let block_len = read_u32(payload, pos - 4) as usize;
        if since < 2 || since > header_version as u16 || block_len < crate::EXT_TRAILER_LEN || block_len > pos {
            return None;
        }
        let block_start = pos - block_len;
        if since == target {
            let end = pos - crate::EXT_TRAILER_LEN;
            return (end - block_start >= min_len).then_some((block_start, end));
        }
        if since > target && (since > max_known || versions.contains(&since)) {
            pos = block_start;
            continue;
        }
        return None;
    }
}

// ---- Lazy views ----

/// Inline element view (scalars / i32-backed enums / struct fixed regions). stride is the element footprint.
#[derive(Clone, Copy)]
pub struct StrideView<'a> {
    pub buf: &'a [u8],
    pub start: usize,
    pub count: usize,
    pub stride: usize,
}

impl<'a> StrideView<'a> {
    pub fn new(buf: &'a [u8], start: usize, count: usize, stride: usize) -> Self {
        StrideView { buf, start, count, stride }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Absolute start of element i.
    pub fn origin_of(&self, i: usize) -> usize {
        assert!(i < self.count, "index {i} out of bounds (len {})", self.count);
        self.start + i * self.stride
    }
}

/// Slot-shaped element view (string/bytes/nested arrays): each element contributes an 8-byte slot.
#[derive(Clone, Copy)]
pub struct SlotListView<'a> {
    pub buf: &'a [u8],
    pub start: usize,
    pub count: usize,
}

impl<'a> SlotListView<'a> {
    pub fn new(buf: &'a [u8], start: usize, count: usize) -> Self {
        SlotListView { buf, start, count }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn slot_of(&self, i: usize) -> (u32, u32) {
        assert!(i < self.count, "index {i} out of bounds (len {})", self.count);
        read_slot(self.buf, self.start + i * SLOT_LEN)
    }
}

// ---- Typed list views ----

/// Scalar element read primitive.
pub trait Prim: Sized {
    const W: usize;
    fn rd(buf: &[u8], off: usize) -> Self;
}

macro_rules! impl_prim {
    ($t:ty, $w:expr, $body:expr) => {
        impl Prim for $t {
            const W: usize = $w;
            fn rd(buf: &[u8], off: usize) -> Self {
                #[allow(clippy::redundant_closure_call)]
                ($body)(buf, off)
            }
        }
    };
}

impl_prim!(u8, 1, |b, o| read_u8(b, o));
impl_prim!(i8, 1, |b, o| read_u8(b, o) as i8);
impl_prim!(bool, 1, |b, o| read_bool(b, o));
impl_prim!(u16, 2, |b, o| read_u16(b, o));
impl_prim!(i16, 2, |b, o| read_i16(b, o));
impl_prim!(u32, 4, |b, o| read_u32(b, o));
impl_prim!(i32, 4, |b, o| read_i32(b, o));
impl_prim!(u64, 8, |b, o| read_u64(b, o));
impl_prim!(i64, 8, |b, o| read_i64(b, o));
impl_prim!(f32, 4, |b, o| read_f32(b, o));
impl_prim!(f64, 8, |b, o| read_f64(b, o));

/// Scalar list view (stride = element width).
#[derive(Clone, Copy)]
pub struct ScalarListView<'a, T: Prim> {
    view: StrideView<'a>,
    _pd: std::marker::PhantomData<fn() -> T>,
}

impl<'a, T: Prim> ScalarListView<'a, T> {
    pub fn new(buf: &'a [u8], start: usize, count: usize) -> Self {
        ScalarListView {
            view: StrideView::new(buf, start, count, T::W),
            _pd: std::marker::PhantomData,
        }
    }

    pub fn len(&self) -> usize {
        self.view.len()
    }

    pub fn is_empty(&self) -> bool {
        self.view.is_empty()
    }

    pub fn get(&self, i: usize) -> T {
        T::rd(self.view.buf, self.view.origin_of(i))
    }
}

/// String list view (elements are reference slots).
#[derive(Clone, Copy)]
pub struct StringListView<'a> {
    view: SlotListView<'a>,
}

impl<'a> StringListView<'a> {
    pub fn new(buf: &'a [u8], start: usize, count: usize) -> Self {
        StringListView { view: SlotListView::new(buf, start, count) }
    }

    pub fn len(&self) -> usize {
        self.view.len()
    }

    pub fn is_empty(&self) -> bool {
        self.view.is_empty()
    }

    pub fn get(&self, i: usize) -> &'a str {
        let (o, l) = self.view.slot_of(i);
        std::str::from_utf8(slot_slice(self.view.buf, o, l))
            .expect("invalid utf-8 in string list element")
    }
}

/// Bytes list view (elements are reference slots).
#[derive(Clone, Copy)]
pub struct BytesListView<'a> {
    view: SlotListView<'a>,
}

impl<'a> BytesListView<'a> {
    pub fn new(buf: &'a [u8], start: usize, count: usize) -> Self {
        BytesListView { view: SlotListView::new(buf, start, count) }
    }

    pub fn len(&self) -> usize {
        self.view.len()
    }

    pub fn is_empty(&self) -> bool {
        self.view.is_empty()
    }

    pub fn get(&self, i: usize) -> &'a [u8] {
        let (o, l) = self.view.slot_of(i);
        slot_slice(self.view.buf, o, l)
    }
}

/// Struct/enum list view (elements are contiguous fixed regions).
#[derive(Clone, Copy)]
pub struct StructListView<'a, T: crate::traits::StructDecode> {
    view: StrideView<'a>,
    _pd: std::marker::PhantomData<fn() -> T>,
}

impl<'a, T: crate::traits::StructDecode> StructListView<'a, T> {
    pub fn new(buf: &'a [u8], start: usize, count: usize) -> Self {
        let stride = <T as crate::traits::Fixed>::FIXED_LEN;
        StructListView { view: StrideView::new(buf, start, count, stride), _pd: std::marker::PhantomData }
    }

    pub fn len(&self) -> usize {
        self.view.len()
    }

    pub fn is_empty(&self) -> bool {
        self.view.is_empty()
    }

    pub fn get(&self, i: usize) -> T::Reader<'a> {
        T::__reader(self.view.buf, self.view.origin_of(i), 0)
    }
}
