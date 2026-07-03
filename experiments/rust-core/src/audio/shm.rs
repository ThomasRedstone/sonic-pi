//! Shared-memory ring reader/writer — a faithful Rust mirror of SuperSonic's
//! audio-buffer protocol, for the Scope.
//!
//! This is the hardest piece of the Rust core (see plan/02-implementation-plan.md,
//! Phase 1/2): reading the audio engine's shared memory with a byte-for-byte
//! `#[repr(C)]` layout and the same atomic ordering, with no tearing on the
//! reader. Proven in the Phase-1 GPUI spike, now folded into the core as its
//! permanent home; the GPUI spike consumes it from here.
//!
//! Layout mirrored from the C++ headers in this repo:
//!   * `app/api/include/api/audio/server_shm.hpp`     (segment header)
//!   * `app/api/include/api/audio/shm_audio_buffer.hpp` (per-slot ring)
//!
//! Protocol summary (audio slot):
//!   * Fixed-layout single-producer / single-consumer ring of interleaved
//!     float audio. Slot 0 is the master output mix.
//!   * 32-byte header, then `data[frames * channels]`.
//!   * `write_position` = total frames ever written (monotone u64). The
//!     producer stores it with Release AFTER writing the block; the consumer
//!     loads it with Acquire BEFORE reading — that pair is the synchronisation.
//!   * The ring wraps modulo `capacity_frames`.
//!
//! The real segment is self-describing: a `shm_segment_header` with a MAGIC and
//! region offsets sits at the front; the audio slots live at `audio_offset`
//! within the blob. We reproduce that discovery flow (validate MAGIC → read
//! offsets → locate slot 0) rather than hardcoding positions.
//!
//! NB: SuperSonic also has a *dedicated* triple-buffered scope path
//! (`shm_scope_buffer` in server_shm.hpp) which is tear-free by publish-by-index.
//! This spike deliberately targets the audio *ring* — it's the fully-specified,
//! harder-concurrency case (wrapping + release/acquire) and also the recording
//! source. Swapping to the triple-buffered path later is strictly easier.

// The raw-pointer accessors below are already `unsafe fn`; in edition 2024 each
// unsafe op inside them would otherwise need its own `unsafe {}` block. The
// whole module is inherently unsafe shm plumbing, so we allow it here.
#![allow(unsafe_op_in_unsafe_fn)]

#[cfg(unix)]
use std::ffi::CString;
use std::io;
use std::mem::offset_of;
use std::sync::atomic::{fence, AtomicI32, AtomicU32, AtomicU64, Ordering};

/// POSIX shm name (a leading '/' is required). The real engine uses
/// `SuperSonic_<port>`; the spike uses its own so it can't clash.
pub const SCOPE_SHM_NAME: &str = "/sonic_scope_spike";

// ── Constants mirrored from shm_audio_buffer.hpp ────────────────────────────
pub const SHM_AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const SHM_AUDIO_SECONDS: u32 = 1;
pub const SHM_AUDIO_FRAMES: u32 = SHM_AUDIO_SAMPLE_RATE * SHM_AUDIO_SECONDS;
pub const SHM_AUDIO_CHANNELS: u32 = 2;
pub const MAX_SHM_AUDIO_BUFFERS: u32 = 4;
pub const SHM_AUDIO_MASTER_SLOT: u32 = 0;

/// Segment header MAGIC (0x5C09E006 = "unified layout + Link metrics").
pub const SEGMENT_MAGIC: u32 = 0x5C09_E006;

/// True once a shared-memory segment named `name` exists — the portable
/// engine-readiness probe (checking `/dev/shm/<name>` only works on Linux;
/// macOS segments have no filesystem presence, Windows uses named sections).
#[cfg(unix)]
pub fn segment_exists(name: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(name) else { return false };
    let fd = unsafe { libc::shm_open(c.as_ptr(), libc::O_RDONLY, 0) };
    if fd >= 0 {
        unsafe { libc::close(fd) };
        true
    } else {
        false
    }
}

/// Windows: probe by opening the named section read-only.
#[cfg(windows)]
pub fn segment_exists(name: &str) -> bool {
    use windows_sys::Win32::System::Memory::{OpenFileMappingW, FILE_MAP_READ};
    let wname = wide_name(name);
    unsafe {
        let h = OpenFileMappingW(FILE_MAP_READ, 0, wname.as_ptr());
        if h.is_null() {
            false
        } else {
            windows_sys::Win32::Foundation::CloseHandle(h);
            true
        }
    }
}

/// Where the blob starts within the segment (past the header). 16-aligned and
/// comfortably larger than `size_of::<ShmSegmentHeader>()`.
const BLOB_OFFSET: u32 = 256;

/// Per-slot ring. `#[repr(C, align(16))]` so `data` lands at offset 32, exactly
/// as the C++ `static_assert(offsetof(shm_audio_buffer, data) == 32)` requires.
#[repr(C, align(16))]
pub struct ShmAudioBuffer {
    pub enabled: AtomicU32,       // 0:  0 = idle, 1 = writes flowing
    pub sample_rate: u32,         // 4
    pub channels: u32,            // 8
    pub capacity_frames: u32,     // 12
    pub write_position: AtomicU64,// 16: total frames written since activation
    pub _padding: [u32; 2],       // 24: pad header to 32 bytes
    pub data: [f32; (SHM_AUDIO_FRAMES * SHM_AUDIO_CHANNELS) as usize], // 32
}

// Compile-time layout checks — the Rust equivalents of the C++ static_asserts.
const _: () = assert!(offset_of!(ShmAudioBuffer, data) == 32);
const _: () = assert!(
    std::mem::size_of::<ShmAudioBuffer>()
        == 32 + (SHM_AUDIO_FRAMES * SHM_AUDIO_CHANNELS) as usize * 4
);
const _: () = assert!(std::mem::align_of::<ShmAudioBuffer>() == 16);

pub const SHM_AUDIO_SLOT_SIZE: u32 = std::mem::size_of::<ShmAudioBuffer>() as u32;

/// Self-describing segment header. Byte-for-byte field order from
/// `server_shm.hpp`'s `shm_segment_header` (all u32). The spike only fills the
/// audio-related fields; the rest are zero.
#[repr(C)]
pub struct ShmSegmentHeader {
    pub magic: u32,
    pub blob_offset: u32,
    pub blob_size: u32,

    pub in_ring_offset: u32,
    pub in_ring_size: u32,
    pub out_ring_offset: u32,
    pub out_ring_size: u32,
    pub debug_ring_offset: u32,
    pub debug_ring_size: u32,
    pub control_offset: u32,

    pub metrics_offset: u32,
    pub metrics_field_count: u32,

    pub node_tree_offset: u32,
    pub node_tree_header_bytes: u32,
    pub node_tree_entry_bytes: u32,
    pub node_tree_max_nodes: u32,

    pub audio_offset: u32,
    pub audio_slot_count: u32,
    pub audio_slot_bytes: u32,

    pub scope_offset: u32,
    pub scope_max: u32,
    pub scope_header_bytes: u32,
    pub scope_slot_bytes: u32,
    pub scope_slot_header: u32,
    pub scope_frames: u32,
    pub scope_channels: u32,

    pub native_stats_offset: u32,
}

// ── Shared-memory mapping (create / open existing) ──────────────────────────
//
// Mirrors `shm_handle` + `shm_open_existing`/`shm_close` in server_shm.hpp,
// including its platform split: POSIX shm_open/mmap vs Windows
// CreateFileMapping/MapViewOfFile. SuperSonic passes segment names WITHOUT a
// leading slash on Windows and prepends "/" itself on POSIX — our callers use
// the POSIX spelling ("/SuperSonic_<port>"), so the Windows half strips it.

/// An mmap'd shared-memory segment. Unmaps (and, if we created it, unlinks) on
/// drop.
#[cfg(unix)]
struct Mapping {
    ptr: *mut u8,
    size: usize,
    fd: libc::c_int,
    name: CString,
    owner: bool,
}

#[cfg(unix)]
impl Mapping {
    fn create(name: &str, size: usize) -> io::Result<Mapping> {
        let cname = CString::new(name).unwrap();
        unsafe {
            // Fresh start: drop any stale segment from a previous run.
            libc::shm_unlink(cname.as_ptr());
            let fd = libc::shm_open(
                cname.as_ptr(),
                libc::O_CREAT | libc::O_RDWR,
                // c_uint, not mode_t: macOS declares shm_open variadic and
                // its mode_t (u16) can't cross varargs; Linux's mode_t IS
                // c_uint, so this cast is correct on both.
                0o600 as libc::c_uint,
            );
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ftruncate(fd, size as libc::off_t) != 0 {
                let e = io::Error::last_os_error();
                libc::close(fd);
                libc::shm_unlink(cname.as_ptr());
                return Err(e);
            }
            let ptr = libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            );
            if ptr == libc::MAP_FAILED {
                let e = io::Error::last_os_error();
                libc::close(fd);
                libc::shm_unlink(cname.as_ptr());
                return Err(e);
            }
            Ok(Mapping { ptr: ptr as *mut u8, size, fd, name: cname, owner: true })
        }
    }

    fn open(name: &str) -> io::Result<Mapping> {
        let cname = CString::new(name).unwrap();
        unsafe {
            let fd = libc::shm_open(cname.as_ptr(), libc::O_RDWR, 0);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let mut st: libc::stat = std::mem::zeroed();
            if libc::fstat(fd, &mut st) != 0 {
                let e = io::Error::last_os_error();
                libc::close(fd);
                return Err(e);
            }
            let size = st.st_size as usize;
            let ptr = libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            );
            if ptr == libc::MAP_FAILED {
                let e = io::Error::last_os_error();
                libc::close(fd);
                return Err(e);
            }
            Ok(Mapping { ptr: ptr as *mut u8, size, fd, name: cname, owner: false })
        }
    }
}

#[cfg(unix)]
impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            if !self.ptr.is_null() {
                libc::munmap(self.ptr as *mut libc::c_void, self.size);
            }
            if self.fd >= 0 {
                libc::close(self.fd);
            }
            if self.owner {
                libc::shm_unlink(self.name.as_ptr());
            }
        }
    }
}

/// Windows half: named file-mapping objects (session-local namespace, same
/// bare names SuperSonic creates). Sections are refcounted kernel objects —
/// no unlink; the last CloseHandle releases them.
#[cfg(windows)]
struct Mapping {
    ptr: *mut u8,
    size: usize,
    handle: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
fn wide_name(name: &str) -> Vec<u16> {
    // POSIX spelling → Windows object name (strip the leading '/').
    let bare = name.strip_prefix('/').unwrap_or(name);
    bare.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
impl Mapping {
    fn create(name: &str, size: usize) -> io::Result<Mapping> {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Memory::{
            CreateFileMappingW, MapViewOfFile, FILE_MAP_ALL_ACCESS, PAGE_READWRITE,
        };
        let wname = wide_name(name);
        unsafe {
            let handle = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_READWRITE,
                (size as u64 >> 32) as u32,
                size as u32,
                wname.as_ptr(),
            );
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, size);
            if view.Value.is_null() {
                let e = io::Error::last_os_error();
                windows_sys::Win32::Foundation::CloseHandle(handle);
                return Err(e);
            }
            Ok(Mapping { ptr: view.Value as *mut u8, size, handle })
        }
    }

    fn open(name: &str) -> io::Result<Mapping> {
        use windows_sys::Win32::System::Memory::{
            MapViewOfFile, OpenFileMappingW, VirtualQuery, FILE_MAP_ALL_ACCESS,
            MEMORY_BASIC_INFORMATION,
        };
        let wname = wide_name(name);
        unsafe {
            let handle = OpenFileMappingW(FILE_MAP_ALL_ACCESS, 0, wname.as_ptr());
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            let view = MapViewOfFile(handle, FILE_MAP_ALL_ACCESS, 0, 0, 0);
            if view.Value.is_null() {
                let e = io::Error::last_os_error();
                windows_sys::Win32::Foundation::CloseHandle(handle);
                return Err(e);
            }
            // Section size: like server_shm.hpp, VirtualQuery's RegionSize
            // (page-rounded — the self-describing header carries the real
            // layout, this only backs the bounds checks).
            let mut info: MEMORY_BASIC_INFORMATION = std::mem::zeroed();
            let n = VirtualQuery(
                view.Value,
                &mut info,
                std::mem::size_of::<MEMORY_BASIC_INFORMATION>(),
            );
            if n == 0 {
                let e = io::Error::last_os_error();
                windows_sys::Win32::System::Memory::UnmapViewOfFile(view);
                windows_sys::Win32::Foundation::CloseHandle(handle);
                return Err(e);
            }
            Ok(Mapping { ptr: view.Value as *mut u8, size: info.RegionSize, handle })
        }
    }
}

#[cfg(windows)]
impl Drop for Mapping {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Memory::{UnmapViewOfFile, MEMORY_MAPPED_VIEW_ADDRESS};
        unsafe {
            if !self.ptr.is_null() {
                UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: self.ptr as *mut core::ffi::c_void,
                });
            }
            if !self.handle.is_null() {
                windows_sys::Win32::Foundation::CloseHandle(self.handle);
            }
        }
    }
}

// ── Raw slot accessors ───────────────────────────────────────────────────────
//
// We never form a `&ShmAudioBuffer` over live shared memory (the writer mutates
// `data` non-atomically, which would be a data race for a Rust reference).
// Instead we compute field pointers and touch the atomics as atomics and the
// float samples via volatile reads/writes.

#[inline]
unsafe fn enabled(slot: *mut u8) -> &'static AtomicU32 {
    &*(slot.add(offset_of!(ShmAudioBuffer, enabled)) as *const AtomicU32)
}
#[inline]
unsafe fn write_position(slot: *mut u8) -> &'static AtomicU64 {
    &*(slot.add(offset_of!(ShmAudioBuffer, write_position)) as *const AtomicU64)
}
#[inline]
unsafe fn read_u32(slot: *mut u8, off: usize) -> u32 {
    (slot.add(off) as *const u32).read_volatile()
}
#[inline]
unsafe fn write_u32(slot: *mut u8, off: usize, v: u32) {
    (slot.add(off) as *mut u32).write_volatile(v);
}
#[inline]
unsafe fn data_ptr(slot: *mut u8) -> *mut f32 {
    slot.add(offset_of!(ShmAudioBuffer, data)) as *mut f32
}

// ── Writer: a stand-in for SuperSonic's audio thread ─────────────────────────

/// Creates the segment, publishes the self-describing header, and activates
/// slot 0 so a reader can attach.
pub struct ScopeWriter {
    _map: Mapping, // kept alive; slot0 points into it (Drop unlinks the segment)
    slot0: *mut u8,
    cap: u32,
    channels: u32,
}

impl ScopeWriter {
    pub fn create(name: &str) -> io::Result<ScopeWriter> {
        let slot_bytes = SHM_AUDIO_SLOT_SIZE;
        let audio_total = MAX_SHM_AUDIO_BUFFERS * slot_bytes;
        let size = (BLOB_OFFSET + audio_total) as usize;
        let map = Mapping::create(name, size)?;

        let base = map.ptr;
        let audio_offset: u32 = 0; // audio region at the very start of the blob
        unsafe {
            // Fill header fields first, then publish MAGIC last with a release
            // fence — mirrors the engine's "release before the MAGIC store", so
            // a reader observing MAGIC sees a fully-initialised header.
            let h = base as *mut ShmSegmentHeader;
            std::ptr::write_bytes(h as *mut u8, 0, std::mem::size_of::<ShmSegmentHeader>());
            (*h).blob_offset = BLOB_OFFSET;
            (*h).blob_size = audio_total;
            (*h).audio_offset = audio_offset;
            (*h).audio_slot_count = MAX_SHM_AUDIO_BUFFERS;
            (*h).audio_slot_bytes = slot_bytes;

            let slot0 = base.add((BLOB_OFFSET + audio_offset) as usize);

            // Activate slot 0 (mirrors shm_audio_buffer_writer::activate).
            write_u32(slot0, offset_of!(ShmAudioBuffer, channels), SHM_AUDIO_CHANNELS);
            write_u32(slot0, offset_of!(ShmAudioBuffer, sample_rate), SHM_AUDIO_SAMPLE_RATE);
            write_u32(slot0, offset_of!(ShmAudioBuffer, capacity_frames), SHM_AUDIO_FRAMES);
            write_position(slot0).store(0, Ordering::Relaxed);
            std::ptr::write_bytes(
                data_ptr(slot0) as *mut u8,
                0,
                (SHM_AUDIO_FRAMES * SHM_AUDIO_CHANNELS) as usize * 4,
            );
            enabled(slot0).store(1, Ordering::Release);

            // Publish the header.
            fence(Ordering::Release);
            (*h).magic = SEGMENT_MAGIC;

            Ok(ScopeWriter {
                _map: map,
                slot0,
                cap: SHM_AUDIO_FRAMES,
                channels: SHM_AUDIO_CHANNELS,
            })
        }
    }

    /// Append `num_frames` of interleaved audio. Real-time-safe shape: modulo
    /// wrap, then a single Release store of `write_position` to publish.
    /// Mirrors `shm_audio_buffer_writer::write_interleaved`.
    pub fn write_interleaved(&mut self, interleaved: &[f32], num_frames: u32) {
        if num_frames == 0 {
            return;
        }
        let ch = self.channels;
        let cap = self.cap;
        debug_assert!(interleaved.len() >= (num_frames * ch) as usize);
        unsafe {
            let pos = write_position(self.slot0).load(Ordering::Relaxed);
            let slot = (pos % cap as u64) as u32;
            let first = (cap - slot).min(num_frames);
            let dst = data_ptr(self.slot0);
            std::ptr::copy_nonoverlapping(
                interleaved.as_ptr(),
                dst.add((slot * ch) as usize),
                (first * ch) as usize,
            );
            if first < num_frames {
                let wrap = num_frames - first;
                std::ptr::copy_nonoverlapping(
                    interleaved.as_ptr().add((first * ch) as usize),
                    dst,
                    (wrap * ch) as usize,
                );
            }
            write_position(self.slot0).store(pos + num_frames as u64, Ordering::Release);
        }
    }

    #[cfg(test)]
    fn writer_position(&self) -> u64 {
        unsafe { write_position(self.slot0).load(Ordering::Acquire) }
    }
}

impl Drop for ScopeWriter {
    fn drop(&mut self) {
        unsafe {
            enabled(self.slot0).store(0, Ordering::Release);
        }
        // Mapping::drop unlinks the segment (we own it).
    }
}

// ── Reader: attaches to an existing segment and reads the latest window ───────

pub struct ScopeReader {
    _map: Mapping, // kept alive; slot0 points into it
    slot0: *mut u8,
}

impl ScopeReader {
    /// Attach to an existing segment: validate MAGIC, read the self-describing
    /// offsets, and locate audio slot 0. Mirrors `server_shared_memory_client`.
    pub fn open(name: &str) -> io::Result<ScopeReader> {
        let map = Mapping::open(name)?;
        let base = map.ptr;
        unsafe {
            let h = base as *const ShmSegmentHeader;
            let magic = (base as *const AtomicU32).as_ref().unwrap().load(Ordering::Acquire);
            if magic != SEGMENT_MAGIC {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "bad shm magic — is the writer running?",
                ));
            }
            // Acquire pairs with the writer's release before the MAGIC store.
            fence(Ordering::Acquire);

            let blob = base.add((*h).blob_offset as usize);
            if (*h).audio_slot_count == 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "no audio slots"));
            }
            let slot0 = blob.add(
                (*h).audio_offset as usize
                    + SHM_AUDIO_MASTER_SLOT as usize * (*h).audio_slot_bytes as usize,
            );
            Ok(ScopeReader { _map: map, slot0 })
        }
    }

    pub fn is_active(&self) -> bool {
        unsafe { enabled(self.slot0).load(Ordering::Acquire) != 0 }
    }

    /// Fill `out` with the most recent `want_frames` of channel 0 (mono scope).
    /// Returns true if live data was produced. Snapshots `write_position` with
    /// Acquire, then reads the window ending there, modulo the ring.
    ///
    /// Tearing: for a scope we want the newest window, not a streamed cursor.
    /// A benign tear is only possible if the writer laps the whole window during
    /// the copy; we re-check `write_position` afterwards and report how far it
    /// moved so a caller could retry. At real rates (a ~1k-frame window vs a
    /// writer producing a few hundred frames per block) this never triggers.
    pub fn read_latest_mono(&self, out: &mut Vec<f32>, want_frames: usize) -> bool {
        out.clear();
        unsafe {
            if enabled(self.slot0).load(Ordering::Acquire) == 0 {
                return false;
            }
            let cap = read_u32(self.slot0, offset_of!(ShmAudioBuffer, capacity_frames)) as u64;
            let ch = read_u32(self.slot0, offset_of!(ShmAudioBuffer, channels)) as u64;
            if cap == 0 || ch == 0 {
                return false;
            }
            let wp = write_position(self.slot0).load(Ordering::Acquire);
            if wp == 0 {
                return false;
            }
            let n = (want_frames as u64).min(cap).min(wp);
            let start = wp - n;
            let data = data_ptr(self.slot0);
            out.reserve(n as usize);
            for i in 0..n {
                let frame = (start + i) % cap;
                let s = data.add((frame * ch) as usize).read_volatile(); // channel 0
                out.push(s);
            }
            // Tear check (see doc comment) — informational only.
            let wp2 = write_position(self.slot0).load(Ordering::Acquire);
            debug_assert!(wp2.wrapping_sub(wp) <= cap.saturating_sub(n) || cap == n);
            true
        }
    }
}

// ── Fixed-inline scope-slot reader (triple-buffered) ─────────────────────────
//
// Mirrors `shm_scope_buffer_reader` in SuperSonic's `server_shm.hpp`: per-slot
// header `{state: atomic u32 (1 = active), channels: u32, stage: atomic i32}`,
// then three *planar* regions of `scope_frames * scope_channels` floats
// (channel 0 first); `stage` indexes the newest published region and the
// writer publishes by advancing it. This is what the engine's ScopeOut2 UGens
// write and the Qt scope reads. The audio *ring* above is the recording tap —
// on native it is idle unless a `supersonic-audio-out` synth runs, so a live
// scope must read this path instead.

const SCOPE_SLOT_STATE_OFF: usize = 0;
const SCOPE_SLOT_STAGE_OFF: usize = 8;

pub struct ScopeSlotReader {
    _map: Mapping,
    slot: *mut u8,
    data_off: u32,
    frames: u32,
    channels: u32,
    last_stage: i32,
}

impl ScopeSlotReader {
    /// Attach to scope slot `index` of segment `name` (e.g. `/SuperSonic_<port>`).
    /// All geometry comes from the self-describing segment header; fails if the
    /// segment has no scope region (e.g. the spike's audio-only fake segment).
    pub fn open(name: &str, index: u32) -> io::Result<ScopeSlotReader> {
        let map = Mapping::open(name)?;
        let base = map.ptr;
        unsafe {
            let magic = (base as *const AtomicU32).as_ref().unwrap().load(Ordering::Acquire);
            if magic != SEGMENT_MAGIC {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "bad shm magic — is the engine running?",
                ));
            }
            // Acquire pairs with the writer's release before the MAGIC store.
            fence(Ordering::Acquire);

            let h = base as *const ShmSegmentHeader;
            let frames = (*h).scope_frames;
            let channels = (*h).scope_channels;
            if index >= (*h).scope_max || frames == 0 || channels == 0 {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "no such scope slot"));
            }
            let regions_bytes = 3usize * (frames as usize) * (channels as usize) * 4;
            if ((*h).scope_slot_header as usize) + regions_bytes > (*h).scope_slot_bytes as usize
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "scope regions don't fit the advertised slot size",
                ));
            }
            let off = (*h).blob_offset as usize
                + (*h).scope_offset as usize
                + (*h).scope_header_bytes as usize
                + index as usize * (*h).scope_slot_bytes as usize;
            if off + (*h).scope_slot_bytes as usize > map.size {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "scope slot out of segment bounds",
                ));
            }
            let slot = base.add(off);
            Ok(ScopeSlotReader {
                _map: map,
                slot,
                data_off: (*h).scope_slot_header,
                frames,
                channels,
                last_stage: -1,
            })
        }
    }

    /// True while a writer (ScopeOut2 synth) owns the slot.
    pub fn is_active(&self) -> bool {
        unsafe {
            (*(self.slot.add(SCOPE_SLOT_STATE_OFF) as *const AtomicU32)).load(Ordering::Acquire)
                == 1
        }
    }

    /// The newest published region's base pointer, or None when the slot is
    /// inactive or nothing new was published since the last pull.
    fn pull_latest_region(&mut self) -> Option<*const f32> {
        unsafe {
            if !self.is_active() {
                return None;
            }
            let stage = (*(self.slot.add(SCOPE_SLOT_STAGE_OFF) as *const AtomicI32))
                .load(Ordering::Acquire);
            if !(0..=2).contains(&stage) || stage == self.last_stage {
                return None;
            }
            self.last_stage = stage;
            let region_floats = (self.frames * self.channels) as usize;
            Some(self.slot.add(self.data_off as usize + stage as usize * region_floats * 4)
                as *const f32)
        }
    }

    /// Pull channel 0 of the newest published region into `out`. Returns false
    /// when the slot is inactive or nothing new was published since the last
    /// pull (the previous contents of `out` are then left untouched, so a
    /// caller can keep drawing the last frame).
    pub fn pull_latest_mono(&mut self, out: &mut Vec<f32>) -> bool {
        let Some(region) = self.pull_latest_region() else { return false };
        unsafe {
            out.clear();
            out.reserve(self.frames as usize);
            // Planar layout: channel 0 is the first `frames` floats.
            for i in 0..self.frames as usize {
                out.push(region.add(i).read_volatile());
            }
        }
        true
    }

    /// Pull channels 0 and 1 of the newest published region (planar: ch1 is
    /// the second `frames` floats). A mono slot mirrors ch0 into `right`.
    /// Same publish/no-change semantics as [`Self::pull_latest_mono`].
    pub fn pull_latest_stereo(&mut self, left: &mut Vec<f32>, right: &mut Vec<f32>) -> bool {
        let Some(region) = self.pull_latest_region() else { return false };
        let frames = self.frames as usize;
        let ch1 = if self.channels >= 2 { frames } else { 0 };
        unsafe {
            left.clear();
            left.reserve(frames);
            right.clear();
            right.reserve(frames);
            for i in 0..frames {
                left.push(region.add(i).read_volatile());
                right.push(region.add(ch1 + i).read_volatile());
            }
        }
        true
    }
}

// ── Metrics reader ────────────────────────────────────────────────────────────
//
// `PerformanceMetrics` (shared_memory.h) is a contiguous array of atomic u32
// fields at `metrics_offset`; `metrics_field_count` publishes how many. Reads
// are independent relaxed loads — fine for a dashboard.

/// Well-known `PerformanceMetrics` field indices (shared_memory.h).
pub mod metrics_idx {
    pub const PROCESS_COUNT: usize = 0;
    pub const MESSAGES_PROCESSED: usize = 1;
    pub const SCHEDULER_QUEUE_DEPTH: usize = 3;
    pub const LINK_PEERS: usize = 27;
    /// Tempo in milli-BPM (bpm × 1000).
    pub const LINK_TEMPO_MBPM: usize = 28;
    pub const LINK_PLAYING: usize = 31;
}

pub struct MetricsReader {
    _map: Mapping,
    fields: *mut u8,
    count: u32,
}

impl MetricsReader {
    /// Attach to the engine segment's metrics region.
    pub fn open(name: &str) -> io::Result<MetricsReader> {
        let map = Mapping::open(name)?;
        let base = map.ptr;
        unsafe {
            let magic = (base as *const AtomicU32).as_ref().unwrap().load(Ordering::Acquire);
            if magic != SEGMENT_MAGIC {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "bad shm magic — is the engine running?",
                ));
            }
            fence(Ordering::Acquire);
            let h = base as *const ShmSegmentHeader;
            let count = (*h).metrics_field_count;
            let off = (*h).blob_offset as usize + (*h).metrics_offset as usize;
            if count == 0 || off + count as usize * 4 > map.size {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "no metrics region"));
            }
            let fields = base.add(off);
            Ok(MetricsReader { _map: map, fields, count })
        }
    }

    pub fn field_count(&self) -> u32 {
        self.count
    }

    /// Read one metrics field (None when out of published range).
    pub fn get(&self, index: usize) -> Option<u32> {
        if index >= self.count as usize {
            return None;
        }
        unsafe {
            Some((*(self.fields.add(index * 4) as *const AtomicU32)).load(Ordering::Relaxed))
        }
    }

    /// Link tempo in BPM (from the milli-BPM field), if published.
    pub fn link_bpm(&self) -> Option<f64> {
        self.get(metrics_idx::LINK_TEMPO_MBPM).map(|m| m as f64 / 1000.0)
    }
}

// ── Node-tree reader ──────────────────────────────────────────────────────────
//
// Mirrors `NodeTreeHeader` + `NodeEntry` from shared_memory.h: 16-byte header
// {node_count, version, dropped_count (atomic u32), pad}, then
// `node_tree_max_nodes` entries of `node_tree_entry_bytes` (72) bytes:
// 6×i32 {id, parent, is_group, prev, next, head}, 32-byte NUL-padded
// def_name, 2×u64 uuid. `id == -1` marks an empty slot.

/// One live node in the engine's mirror tree.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeInfo {
    pub id: i32,
    pub parent_id: i32,
    pub is_group: bool,
    /// Synthdef name for synths, "group" for groups.
    pub name: String,
}

pub struct NodeTreeReader {
    _map: Mapping,
    header: *mut u8,
    entries: *mut u8,
    entry_bytes: usize,
    max_nodes: usize,
}

impl NodeTreeReader {
    pub fn open(name: &str) -> io::Result<NodeTreeReader> {
        let map = Mapping::open(name)?;
        let base = map.ptr;
        unsafe {
            let magic = (base as *const AtomicU32).as_ref().unwrap().load(Ordering::Acquire);
            if magic != SEGMENT_MAGIC {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "bad shm magic — is the engine running?",
                ));
            }
            fence(Ordering::Acquire);
            let h = base as *const ShmSegmentHeader;
            let entry_bytes = (*h).node_tree_entry_bytes as usize;
            let max_nodes = (*h).node_tree_max_nodes as usize;
            let header_bytes = (*h).node_tree_header_bytes as usize;
            let off = (*h).blob_offset as usize + (*h).node_tree_offset as usize;
            if entry_bytes < 56 || max_nodes == 0 || off + header_bytes + max_nodes * entry_bytes > map.size
            {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "no node-tree region"));
            }
            let header = base.add(off);
            let entries = header.add(header_bytes);
            Ok(NodeTreeReader { _map: map, header, entries, entry_bytes, max_nodes })
        }
    }

    pub fn version(&self) -> u32 {
        unsafe { (*(self.header.add(4) as *const AtomicU32)).load(Ordering::Acquire) }
    }

    pub fn node_count(&self) -> u32 {
        unsafe { (*(self.header as *const AtomicU32)).load(Ordering::Acquire) }
    }

    /// Snapshot the live nodes (empty slots skipped). Best-effort: a write
    /// racing the scan can tear a row — fine for a display, and `version()`
    /// lets callers re-read when it moved.
    pub fn read_nodes(&self) -> Vec<NodeInfo> {
        let mut out = Vec::new();
        unsafe {
            for i in 0..self.max_nodes {
                let e = self.entries.add(i * self.entry_bytes);
                let id = (e as *const i32).read_volatile();
                if id == -1 {
                    continue;
                }
                let parent_id = (e.add(4) as *const i32).read_volatile();
                let is_group = (e.add(8) as *const i32).read_volatile() != 0;
                let name_bytes = std::slice::from_raw_parts(e.add(24), 32);
                let len = name_bytes.iter().position(|&b| b == 0).unwrap_or(32);
                let name = String::from_utf8_lossy(&name_bytes[..len]).into_owned();
                out.push(NodeInfo { id, parent_id, is_group, name });
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Writes `frames` of ch0=`v0` (ch1=0) as one interleaved block.
    fn write_const(w: &mut ScopeWriter, frames: u32, v0: f32) {
        let ch = SHM_AUDIO_CHANNELS as usize;
        let mut block = vec![0f32; frames as usize * ch];
        for f in 0..frames as usize {
            block[f * ch] = v0;
        }
        w.write_interleaved(&block, frames);
    }

    #[test]
    fn ring_roundtrip() {
        let name = "/sonic_scope_spike_test_rt";
        let mut w = ScopeWriter::create(name).unwrap();
        write_const(&mut w, 256, 0.5);

        let r = ScopeReader::open(name).unwrap();
        assert!(r.is_active());
        let mut out = Vec::new();
        assert!(r.read_latest_mono(&mut out, 1024));
        // Only 256 frames have been produced, so that's all we can read.
        assert_eq!(out.len(), 256);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6), "got {out:?}");
    }

    #[test]
    fn ring_reports_latest_window() {
        let name = "/sonic_scope_spike_test_win";
        let mut w = ScopeWriter::create(name).unwrap();
        write_const(&mut w, 100, 1.0);
        write_const(&mut w, 100, 2.0); // newest

        let r = ScopeReader::open(name).unwrap();
        let mut out = Vec::new();
        assert!(r.read_latest_mono(&mut out, 50));
        assert_eq!(out.len(), 50);
        // The most recent 50 frames must come from the 2.0 block.
        assert!(out.iter().all(|&s| (s - 2.0).abs() < 1e-6), "got {out:?}");
    }

    #[test]
    fn ring_wraps_past_capacity() {
        let name = "/sonic_scope_spike_test_wrap";
        let mut w = ScopeWriter::create(name).unwrap();
        let cap = SHM_AUDIO_FRAMES;
        let chunk = 1000u32;
        let total = cap + 1000; // force a wrap
        let mut n = 0u32;
        let mut v = 0f32;
        while n < total {
            let f = chunk.min(total - n);
            write_const(&mut w, f, v);
            n += f;
            v += 1.0;
        }
        assert_eq!(w.writer_position(), total as u64);

        let r = ScopeReader::open(name).unwrap();
        let mut out = Vec::new();
        assert!(r.read_latest_mono(&mut out, 8));
        assert_eq!(out.len(), 8);
        // Latest samples belong to the final block written (value v-1).
        let last = v - 1.0;
        assert!(out.iter().all(|&s| (s - last).abs() < 1e-6), "out={out:?} last={last}");
    }

    #[test]
    fn scope_slot_reader_pulls_published_stages() {
        // Hand-build a minimal segment with a scope region: header + one slot,
        // exercising exactly the discovery flow the real engine publishes.
        let name = "/sonic_scope_slot_test";
        let frames = 64u32;
        let channels = 2u32;
        let slot_header = 16u32;
        let slot_bytes = slot_header + 3 * frames * channels * 4;
        let scope_header_bytes = 16u32;
        let scope_off = 1024u32; // within the blob
        let size = BLOB_OFFSET as usize
            + scope_off as usize
            + scope_header_bytes as usize
            + slot_bytes as usize;

        let map = Mapping::create(name, size).unwrap();
        unsafe {
            let h = map.ptr as *mut ShmSegmentHeader;
            (*h).blob_offset = BLOB_OFFSET;
            (*h).scope_offset = scope_off;
            (*h).scope_max = 1;
            (*h).scope_header_bytes = scope_header_bytes;
            (*h).scope_slot_bytes = slot_bytes;
            (*h).scope_slot_header = slot_header;
            (*h).scope_frames = frames;
            (*h).scope_channels = channels;

            let slot = map
                .ptr
                .add(BLOB_OFFSET as usize + scope_off as usize + scope_header_bytes as usize);
            // Publish stage 1 with ch0 = 0.25 (planar: ch0 first).
            let region1 = slot.add(slot_header as usize + (frames * channels) as usize * 4)
                as *mut f32;
            for i in 0..frames as usize {
                region1.add(i).write(0.25);
            }
            (*(slot.add(SCOPE_SLOT_STAGE_OFF) as *mut AtomicI32)).store(1, Ordering::Release);
            (*(slot as *mut AtomicU32)).store(1, Ordering::Release); // state = active
            // MAGIC last, as the engine's publish() does.
            (*(map.ptr as *mut AtomicU32)).store(SEGMENT_MAGIC, Ordering::Release);
        }

        let mut r = ScopeSlotReader::open(name, 0).unwrap();
        assert!(r.is_active());
        let mut out = Vec::new();
        assert!(r.pull_latest_mono(&mut out), "expected a publish on first pull");
        assert_eq!(out.len(), frames as usize);
        assert!(out.iter().all(|&s| (s - 0.25).abs() < 1e-6), "got {out:?}");

        // Same stage again → no new data, buffer untouched.
        assert!(!r.pull_latest_mono(&mut out));
        assert_eq!(out.len(), frames as usize);
        let mut left = Vec::new();
        let mut right = Vec::new();
        assert!(!r.pull_latest_stereo(&mut left, &mut right), "same stage must not re-pull");

        // Publish stage 2 with distinct planar channels: ch0 = 0.5, ch1 = -0.5.
        unsafe {
            let slot = map
                .ptr
                .add(BLOB_OFFSET as usize + scope_off as usize + scope_header_bytes as usize);
            let region2 = slot.add(slot_header as usize + 2 * (frames * channels) as usize * 4)
                as *mut f32;
            for i in 0..frames as usize {
                region2.add(i).write(0.5); // ch0 plane
                region2.add(frames as usize + i).write(-0.5); // ch1 plane
            }
            (*(slot.add(SCOPE_SLOT_STAGE_OFF) as *mut AtomicI32)).store(2, Ordering::Release);
        }
        assert!(r.pull_latest_stereo(&mut left, &mut right));
        assert_eq!((left.len(), right.len()), (frames as usize, frames as usize));
        assert!(left.iter().all(|&s| (s - 0.5).abs() < 1e-6), "left {left:?}");
        assert!(right.iter().all(|&s| (s + 0.5).abs() < 1e-6), "right {right:?}");

        // Slot 1 doesn't exist; opening it must fail cleanly.
        assert!(ScopeSlotReader::open(name, 1).is_err());
    }

    #[test]
    fn node_tree_reader_lists_live_nodes() {
        let name = "/sonic_nodetree_test";
        let entry_bytes = 72usize;
        let header_bytes = 16usize;
        let max_nodes = 8usize;
        let tree_off = 256u32;
        let size =
            BLOB_OFFSET as usize + tree_off as usize + header_bytes + max_nodes * entry_bytes;

        let map = Mapping::create(name, size).unwrap();
        unsafe {
            let h = map.ptr as *mut ShmSegmentHeader;
            (*h).blob_offset = BLOB_OFFSET;
            (*h).node_tree_offset = tree_off;
            (*h).node_tree_header_bytes = header_bytes as u32;
            (*h).node_tree_entry_bytes = entry_bytes as u32;
            (*h).node_tree_max_nodes = max_nodes as u32;

            let tree = map.ptr.add(BLOB_OFFSET as usize + tree_off as usize);
            (*(tree as *mut AtomicU32)).store(2, Ordering::Relaxed); // node_count
            (*(tree.add(4) as *mut AtomicU32)).store(7, Ordering::Relaxed); // version
            let entries = tree.add(header_bytes);
            // All slots empty (id = -1)…
            for i in 0..max_nodes {
                (entries.add(i * entry_bytes) as *mut i32).write(-1);
            }
            // …except a group and a synth.
            let e0 = entries;
            (e0 as *mut i32).write(0); // id
            (e0.add(4) as *mut i32).write(-1); // parent: root
            (e0.add(8) as *mut i32).write(1); // is_group
            e0.add(24).copy_from(b"group\0".as_ptr(), 6);
            let e1 = entries.add(3 * entry_bytes); // sparse slot
            (e1 as *mut i32).write(1001);
            (e1.add(4) as *mut i32).write(0);
            (e1.add(8) as *mut i32).write(0);
            e1.add(24).copy_from(b"sonic-pi-beep\0".as_ptr(), 14);

            (*(map.ptr as *mut AtomicU32)).store(SEGMENT_MAGIC, Ordering::Release);
        }

        let r = NodeTreeReader::open(name).unwrap();
        assert_eq!(r.node_count(), 2);
        assert_eq!(r.version(), 7);
        let nodes = r.read_nodes();
        assert_eq!(nodes.len(), 2);
        assert!(nodes[0].is_group && nodes[0].name == "group" && nodes[0].parent_id == -1);
        assert_eq!(nodes[1].name, "sonic-pi-beep");
        assert_eq!(nodes[1].parent_id, 0);
        assert!(!nodes[1].is_group);
    }

    #[test]
    fn readers_reject_missing_segments_and_bad_magic() {
        // Missing segment: every reader errors cleanly.
        assert!(ScopeReader::open("/sonic_oxide_missing_seg").is_err());
        assert!(ScopeSlotReader::open("/sonic_oxide_missing_seg", 0).is_err());
        assert!(MetricsReader::open("/sonic_oxide_missing_seg").is_err());
        assert!(NodeTreeReader::open("/sonic_oxide_missing_seg").is_err());

        // Present but unpublished (magic zero): same story.
        let name = "/sonic_oxide_bad_magic_test";
        let _map = Mapping::create(name, 4096).unwrap();
        assert!(ScopeReader::open(name).is_err());
        assert!(ScopeSlotReader::open(name, 0).is_err());
        assert!(MetricsReader::open(name).is_err());
        assert!(NodeTreeReader::open(name).is_err());
    }

    #[test]
    fn scope_reader_rejects_a_segment_without_audio_slots() {
        let name = "/sonic_oxide_no_slots_test";
        let map = Mapping::create(name, 4096).unwrap();
        unsafe {
            let h = map.ptr as *mut ShmSegmentHeader;
            (*h).blob_offset = BLOB_OFFSET;
            (*h).audio_slot_count = 0;
            (*(map.ptr as *mut AtomicU32)).store(SEGMENT_MAGIC, Ordering::Release);
        }
        assert!(ScopeReader::open(name).is_err());
        // And a scope-slot geometry that can't fit its regions is rejected.
        unsafe {
            let h = map.ptr as *mut ShmSegmentHeader;
            (*h).scope_max = 1;
            (*h).scope_frames = 1024;
            (*h).scope_channels = 2;
            (*h).scope_slot_bytes = 64; // far too small for 3 regions
            (*h).scope_slot_header = 16;
        }
        assert!(ScopeSlotReader::open(name, 0).is_err());
    }

    #[test]
    fn metrics_reader_reads_published_fields() {
        let name = "/sonic_metrics_test";
        let count = 37u32;
        let metrics_off = 512u32;
        let size = BLOB_OFFSET as usize + metrics_off as usize + count as usize * 4;

        let map = Mapping::create(name, size).unwrap();
        unsafe {
            let h = map.ptr as *mut ShmSegmentHeader;
            (*h).blob_offset = BLOB_OFFSET;
            (*h).metrics_offset = metrics_off;
            (*h).metrics_field_count = count;
            let fields = map.ptr.add(BLOB_OFFSET as usize + metrics_off as usize) as *mut u32;
            fields.add(metrics_idx::PROCESS_COUNT).write(4242);
            fields.add(metrics_idx::LINK_TEMPO_MBPM).write(123_500); // 123.5 BPM
            fields.add(metrics_idx::LINK_PEERS).write(2);
            (*(map.ptr as *mut AtomicU32)).store(SEGMENT_MAGIC, Ordering::Release);
        }

        let r = MetricsReader::open(name).unwrap();
        assert_eq!(r.field_count(), count);
        assert_eq!(r.get(metrics_idx::PROCESS_COUNT), Some(4242));
        assert_eq!(r.get(metrics_idx::LINK_PEERS), Some(2));
        assert_eq!(r.link_bpm(), Some(123.5));
        assert_eq!(r.get(999), None);
    }
}
