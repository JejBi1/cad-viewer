//! Vertex data on the GPU in several buffers, and buffer creation that reports a failure instead of panicking.
//!
//! A device allows a buffer of at most `limits().max_buffer_size` bytes (256 MiB unless the application asked for more),
//! and `Device::create_buffer` panics when a buffer is larger or the memory runs out. A drawing of 10 000 texts has
//! 24 million fill vertices (289 MB); a mesh of millions of triangles is as big. The data is therefore cut into buffers
//! of at most [`chunk_bytes`], in the order of the drawing, and a range of the data is drawn as one draw call per buffer
//! it touches ([`split_range`]): the order of everything that is drawn stays what it was.
use bytemuck::Pod;
use eframe::wgpu;

use crate::gpu::block_on;

/// Message when a buffer could not be created (memory of the graphics card, size limit of the device): the application
/// shows it as a warning and the part of the data that is missing is not drawn.
pub const GPU_BUFFER_WARNING: &str =
    "Výkres se nevešel celý do paměti grafické karty, část geometrie se nezobrazí. Zavřete jiné programy, které grafickou kartu využívají, nebo zjednodušte výkres.";

/// Message when a buffer that is made again for a frame or a selection (overlays, the highlight of an entity, the
/// instances of a model) could not be created: what it holds is not drawn, nothing else is lost.
pub const GPU_DYNAMIC_WARNING: &str =
    "Část zobrazení (zvýraznění, překryvy nebo instance dílů) se nevešla do paměti grafické karty a nezobrazí se. Zavřete jiné programy, které grafickou kartu využívají, nebo zjednodušte výkres.";

/// Largest buffer this module creates, whatever the device would allow.
pub const MAX_CHUNK_BYTES: u64 = 128 << 20;

/// Runs `f` (a creation of a buffer, a texture) with the error scopes of the device pushed: a size over the limit or an
/// exhausted memory is an `Err` with the reason instead of the panic that the uncaptured error handler of the device
/// makes of it. The thing `f` made is dropped by the caller when this is an `Err`.
pub fn guarded<T>(device: &wgpu::Device, what: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    let oom = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let made = f();
    let v = block_on(validation.pop());
    let o = block_on(oom.pop());
    match v.or(o) {
        Some(e) => Err(format!("{what}: {e}")),
        None => Ok(made),
    }
}

/// Bytes one buffer of vertex data may have: half of what the device allows for one buffer (the other half stays for the
/// allocator), at most [`MAX_CHUNK_BYTES`].
pub fn chunk_bytes(device: &wgpu::Device) -> u64 {
    (device.limits().max_buffer_size / 2).clamp(1 << 20, MAX_CHUNK_BYTES)
}

/// Items of `item_size` bytes that fit into a buffer of `bytes`, rounded down to a multiple of `group` (3 for the corners of
/// triangles, so that no triangle is split between two buffers). At least one group.
pub fn items_per_buffer(bytes: u64, item_size: usize, group: usize) -> usize {
    let group = group.max(1);
    let n = (bytes / item_size.max(1) as u64) as usize;
    (n / group * group).max(group)
}

/// The pieces of the range `first..first + count` of a sequence cut into buffers of `cap` items: (buffer, first item in
/// the buffer, number of items), in order. An empty range gives nothing.
pub fn split_range(first: usize, count: usize, cap: usize) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let cap = cap.max(1);
    let end = first.saturating_add(count);
    let mut at = first;
    while at < end {
        let buf = at / cap;
        let local = at - buf * cap;
        let n = (cap - local).min(end - at);
        out.push((buf, local, n));
        at += n;
    }
    out
}

/// A buffer holding `bytes` (padded to the 4 bytes a buffer needs). `Err` carries the reason when the size is over the
/// limit of the device or the memory is exhausted; it never panics (the error scopes of the device take the error). The
/// buffer is filled through its mapping at creation, so no queue is needed.
pub fn try_buffer(device: &wgpu::Device, label: &str, bytes: &[u8], usage: wgpu::BufferUsages) -> Result<wgpu::Buffer, String> {
    try_buffer_limited(device, label, bytes, usage, None)
}

/// `try_buffer` with a smaller size limit than the device has (`Some`): tests of a device that refuses buffers.
pub fn try_buffer_limited(device: &wgpu::Device, label: &str, bytes: &[u8], usage: wgpu::BufferUsages, limit: Option<u64>) -> Result<wgpu::Buffer, String> {
    let size = (bytes.len() as u64).next_multiple_of(wgpu::COPY_BUFFER_ALIGNMENT).max(wgpu::COPY_BUFFER_ALIGNMENT);
    try_buffer_sized(device, label, size, bytes, usage, limit)
}

/// `try_buffer` for a buffer of `size` bytes whose first bytes are `bytes` (tests ask for sizes the device refuses without
/// having that much data).
pub fn try_buffer_sized(device: &wgpu::Device, label: &str, size: u64, bytes: &[u8], usage: wgpu::BufferUsages, limit: Option<u64>) -> Result<wgpu::Buffer, String> {
    let limit = limit.unwrap_or(u64::MAX).min(device.limits().max_buffer_size);
    if size > limit {
        return Err(format!("buffer '{label}' of {size} bytes is over the limit of the device ({limit} bytes)"));
    }
    let buffer = guarded(device, &format!("buffer '{label}' of {size} bytes"), || {
        device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: true })
    })?;
    match buffer.get_mapped_range_mut(..) {
        Ok(mut view) => view.slice(..bytes.len()).copy_from_slice(bytes),
        Err(e) => return Err(format!("buffer '{label}': {e}")),
    }
    buffer.unmap();
    Ok(buffer)
}

/// One piece of an indexed list that was cut into several: its own vertices, indices that refer to them, and the position of
/// its first index in the whole list.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshPiece<V> {
    pub vertices: Vec<V>,
    pub indices: Vec<u32>,
    pub first_index: usize,
}

/// Cuts an indexed list of primitives (`stride` indices each: 3 for triangles, 2 for line segments) into pieces of at most
/// `max_vertices` vertices and `max_indices` indices. The pieces hold every primitive once and in order, so the index `i`
/// of the whole list is the index `i - first_index` of the piece that contains it: ranges of the list (the triangles of
/// one face) are drawn from the pieces they touch. A vertex that two pieces need is in both. Indices that point past the
/// vertices refer to the first vertex (the list keeps its length, `build_proto` already replaced such triangles).
pub fn split_indexed<V: Copy>(vertices: &[V], indices: &[u32], stride: usize, max_vertices: usize, max_indices: usize) -> Vec<MeshPiece<V>> {
    let stride = stride.max(1);
    if vertices.is_empty() {
        return Vec::new();
    }
    let max_vertices = max_vertices.max(stride);
    let max_indices = (max_indices / stride * stride).max(stride);
    let mut pieces: Vec<MeshPiece<V>> = Vec::new();
    // local index of a vertex in the current piece, valid when `stamp` is the number of the piece
    let mut local = vec![0u32; vertices.len()];
    let mut stamp = vec![u32::MAX; vertices.len()];
    let mut cur = MeshPiece { vertices: Vec::new(), indices: Vec::new(), first_index: 0 };
    let mut epoch = 0u32;
    for prim in indices.chunks(stride) {
        // a trailing partial primitive is not drawn, as the GPU would not
        if prim.len() < stride {
            break;
        }
        let need = prim.iter().filter(|&&i| stamp[if (i as usize) < vertices.len() { i as usize } else { 0 }] != epoch).count();
        if !cur.indices.is_empty() && (cur.vertices.len() + need > max_vertices || cur.indices.len() + stride > max_indices) {
            let next_first = cur.first_index + cur.indices.len();
            pieces.push(std::mem::replace(&mut cur, MeshPiece { vertices: Vec::new(), indices: Vec::new(), first_index: next_first }));
            epoch += 1;
        }
        for &i in prim {
            let g = if (i as usize) < vertices.len() { i as usize } else { 0 };
            if stamp[g] != epoch {
                stamp[g] = epoch;
                local[g] = cur.vertices.len() as u32;
                cur.vertices.push(vertices[g]);
            }
            cur.indices.push(local[g]);
        }
    }
    if !cur.indices.is_empty() {
        pieces.push(cur);
    }
    pieces
}

/// Data in buffers of `cap` items each (the last one may hold fewer); a buffer that could not be created is `None`.
#[derive(Default)]
pub struct Chunked {
    pub bufs: Vec<Option<wgpu::Buffer>>,
    pub cap: usize,
    /// Items in all buffers together.
    pub len: usize,
}

impl Chunked {
    /// The buffer that holds items of the range `first..first + count`, each piece as (buffer, first item in it, items).
    /// Buffers that are missing are left out.
    pub fn pieces(&self, first: usize, count: usize) -> impl Iterator<Item = (&wgpu::Buffer, usize, usize)> + '_ {
        split_range(first, count, self.cap.max(1)).into_iter().filter_map(|(b, f, n)| self.bufs.get(b).and_then(|x| x.as_ref()).map(|buf| (buf, f, n)))
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Items that are in a buffer that exists (`len` counts the items of buffers that could not be made as well).
    pub fn present_len(&self) -> usize {
        let cap = self.cap.max(1);
        self.bufs.iter().enumerate().filter(|(_, b)| b.is_some()).map(|(i, _)| cap.min(self.len.saturating_sub(i * cap))).sum()
    }
}

/// Collects items and cuts them into buffers as they come: only one buffer of items is held in memory.
pub struct ChunkWriter<'a, T: Pod> {
    device: &'a wgpu::Device,
    label: &'static str,
    usage: wgpu::BufferUsages,
    cap: usize,
    cur: Vec<T>,
    out: Chunked,
    /// First error, if any buffer failed.
    pub error: Option<String>,
    failed: usize,
    /// Smaller buffers than the device takes are refused (tests).
    limit: Option<u64>,
    /// The queue that is submitted to after each buffer (`with_queue`).
    queue: Option<wgpu::Queue>,
}

impl<'a, T: Pod> ChunkWriter<'a, T> {
    pub fn new(device: &'a wgpu::Device, label: &'static str, usage: wgpu::BufferUsages, cap: usize) -> Self {
        ChunkWriter {
            device,
            label,
            usage,
            cap: cap.max(1),
            cur: Vec::new(),
            out: Chunked { bufs: Vec::new(), cap: cap.max(1), len: 0 },
            error: None,
            failed: 0,
            limit: None,
            queue: None,
        }
    }

    /// An empty submission to `queue` after each buffer that was made. A buffer that is filled through its mapping at creation
    /// is written from a staging buffer of its own size, which the device keeps until the next submission: for a scene of
    /// 600 MB that is another 600 MB until the first frame is drawn. Submitting after each buffer lets the device let go of the
    /// staging buffer of every one before the next is made.
    pub fn with_queue(mut self, queue: Option<&wgpu::Queue>) -> Self {
        self.queue = queue.cloned();
        self
    }

    /// The writer behaves as if the device refused buffers over `limit` bytes (tests of the failure path).
    pub fn with_limit(mut self, limit: Option<u64>) -> Self {
        self.limit = limit;
        self
    }

    /// Items pushed so far (the index the next one gets).
    pub fn len(&self) -> usize {
        self.out.len + self.cur.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn push(&mut self, item: T) {
        if self.cur.len() == self.cap {
            self.flush();
        }
        self.cur.push(item);
    }

    fn flush(&mut self) {
        if self.cur.is_empty() {
            return;
        }
        let n = self.cur.len();
        match try_buffer_limited(self.device, self.label, bytemuck::cast_slice(&self.cur), self.usage, self.limit) {
            Ok(b) => {
                self.out.bufs.push(Some(b));
                if let Some(q) = &self.queue {
                    q.submit(std::iter::empty::<wgpu::CommandBuffer>());
                }
            }
            Err(e) => {
                self.failed += 1;
                self.error.get_or_insert(e);
                self.out.bufs.push(None);
            }
        }
        self.out.len += n;
        self.cur.clear();
    }

    /// The buffers, and how many of them could not be created.
    pub fn finish(mut self) -> (Chunked, usize, Option<String>) {
        self.flush();
        (self.out, self.failed, self.error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_are_cut_at_the_buffer_borders_in_order() {
        assert_eq!(split_range(0, 0, 10), vec![]);
        assert_eq!(split_range(3, 4, 10), vec![(0, 3, 4)]);
        assert_eq!(split_range(8, 5, 10), vec![(0, 8, 2), (1, 0, 3)]);
        assert_eq!(split_range(0, 25, 10), vec![(0, 0, 10), (1, 0, 10), (2, 0, 5)]);
        assert_eq!(split_range(10, 10, 10), vec![(1, 0, 10)]);
        assert_eq!(split_range(9, 2, 10), vec![(0, 9, 1), (1, 0, 1)]);
        // every item of every range is drawn exactly once, in order
        for (first, count, cap) in [(0usize, 100usize, 7usize), (13, 57, 5), (6, 1, 3), (999, 1001, 64)] {
            let pieces = split_range(first, count, cap);
            let flat: Vec<usize> = pieces.iter().flat_map(|&(b, f, n)| (0..n).map(move |i| b * cap + f + i)).collect();
            assert_eq!(flat, (first..first + count).collect::<Vec<_>>());
            assert!(pieces.iter().all(|&(_, f, n)| f + n <= cap));
        }
        // a zero capacity does not loop
        assert_eq!(split_range(0, 3, 0), vec![(0, 0, 1), (1, 0, 1), (2, 0, 1)]);
    }

    #[test]
    fn buffers_hold_whole_triangles_and_at_least_one_item() {
        assert_eq!(items_per_buffer(120, 12, 3), 9);
        assert_eq!(items_per_buffer(121, 12, 3), 9);
        assert_eq!(items_per_buffer(131, 12, 3), 9);
        assert_eq!(items_per_buffer(132, 12, 3), 9);
        assert_eq!(items_per_buffer(144, 12, 3), 12);
        assert_eq!(items_per_buffer(100, 24, 1), 4);
        assert_eq!(items_per_buffer(1, 24, 1), 1);
        assert_eq!(items_per_buffer(1, 12, 3), 3);
        // the 128 MiB buffers of the default device: whole triangles of the fill vertices of the 2D scene
        let n = items_per_buffer(MAX_CHUNK_BYTES, 12, 3);
        assert_eq!(n % 3, 0);
        assert!(n as u64 * 12 <= MAX_CHUNK_BYTES && (n as u64 + 3) * 12 > MAX_CHUNK_BYTES);
    }

    #[test]
    fn an_indexed_list_is_cut_into_pieces_that_cover_it_once_in_order() {
        // 6 vertices on a strip, 4 triangles
        let vertices: Vec<u32> = (100..106).collect();
        let indices = [0u32, 1, 2, 1, 2, 3, 2, 3, 4, 3, 4, 5];
        let pieces = split_indexed(&vertices, &indices, 3, 4, 100);
        assert!(pieces.len() >= 2);
        // every piece is small enough, indices point into the piece, and what they give is what the whole list gives
        let mut at = 0;
        for p in &pieces {
            assert_eq!(p.first_index, at);
            assert!(p.vertices.len() <= 4 && p.indices.len() % 3 == 0);
            assert!(p.indices.iter().all(|&i| (i as usize) < p.vertices.len()));
            for (k, &i) in p.indices.iter().enumerate() {
                assert_eq!(p.vertices[i as usize], vertices[indices[at + k] as usize], "index {}", at + k);
            }
            at += p.indices.len();
        }
        assert_eq!(at, indices.len());
        // segments (two indices each) and a limit on the indices
        let seg = split_indexed(&vertices, &[0, 1, 1, 2, 2, 3, 3, 4, 4, 5], 2, 100, 4);
        assert_eq!(seg.iter().map(|p| p.indices.len()).collect::<Vec<_>>(), vec![4, 4, 2]);
        assert_eq!(seg.iter().map(|p| p.first_index).collect::<Vec<_>>(), vec![0, 4, 8]);
        // everything fits: one piece, the data as it is
        let one = split_indexed(&vertices, &indices, 3, 100, 100);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].indices, indices.to_vec());
        // nothing, and an index past the vertices (kept as a degenerate primitive on the first vertex)
        assert!(split_indexed::<u32>(&[], &[0, 1, 2], 3, 10, 10).is_empty());
        let bad = split_indexed(&vertices, &[0, 1, 99], 3, 10, 10);
        assert_eq!(bad[0].indices.len(), 3);
    }
}
