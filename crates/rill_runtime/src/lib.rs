//! The Rill runtime, linked into every compiled Rill executable.
//!
//! Deliberately `#![no_std]` so binaries stay C-sized: no Rust formatting
//! machinery, no panic/unwind tables — just direct libSystem calls. Together
//! with `-Wl,-dead_strip` + `strip` this keeps a hello-world in the tens of
//! kilobytes.
//!
//! M2 scope: printing intrinsics and string equality. Later milestones add
//! the reference-counting allocator (M3) and the green-thread scheduler and
//! channels (M4).
//!
//! Three shapes of host, not two. A hosted target has threads, mapped memory
//! and sockets; wasm has none of the three and gets stubs instead. A bare
//! Cortex-M target wants exactly the same stubs for exactly the same reasons —
//! no OS to ask for a thread, no address space to map, no network — so it
//! shares them rather than growing a second set, which is why the `wasm` arms
//! below read `any(target_family = "wasm", target_os = "none")`. Where the two
//! genuinely differ, they differ by name: `target_os = "none"` has 64-bit
//! atomics missing from the instruction set, and no `mmap` to fall back on.

#![no_std]

// The five file calls are the same five everywhere; Microsoft's library just
// spells them with an underscore, having decided the plain names were not
// theirs to define. Same arguments, same meanings, different symbol.
extern "C" {
    #[cfg_attr(windows, link_name = "_write")]
    fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    #[cfg_attr(windows, link_name = "_read")]
    fn read(fd: i32, buf: *mut u8, count: usize) -> isize;
    #[cfg_attr(windows, link_name = "_open")]
    fn open(path: *const u8, flags: i32, ...) -> i32;
    #[cfg_attr(windows, link_name = "_close")]
    fn close(fd: i32) -> i32;
    #[cfg_attr(windows, link_name = "_lseeki64")]
    fn lseek(fd: i32, offset: i64, whence: i32) -> i64;
    fn snprintf(buf: *mut u8, size: usize, fmt: *const u8, ...) -> i32;
    fn abort() -> !;
    fn isatty(fd: i32) -> i32;
    fn atexit(f: extern "C" fn()) -> i32;
    fn exit(code: i32) -> !;
    fn malloc(size: usize) -> *mut u8;
    fn calloc(count: usize, size: usize) -> *mut u8;
    fn memset(p: *mut u8, c: i32, n: usize) -> *mut u8;
    fn free(ptr: *mut u8);
    fn getenv(name: *const u8) -> *const u8;
    fn strtod(s: *const u8, end: *mut *const u8) -> f64;
    // Searching a byte at a time is what a naive `find` does; `memchr` is
    // vectorized in every C library worth linking against, and standard
    // everywhere this runtime is built — including wasi and Windows, which
    // `memmem` is not.
    fn memchr(s: *const u8, c: i32, n: usize) -> *const u8;
    fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32;
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { die() }
}

/// Satisfies the linker when the staticlib is built with unwinding enabled;
/// nothing here ever unwinds.
#[no_mangle]
pub extern "C" fn rust_eh_personality() {}

// ---- standard output ------------------------------------------------------
//
// Buffered, the way C's stdout is: a line at a time when it is a terminal,
// 64 KB at a time otherwise, and flushed before anything that waits — the end
// of the program, `exit`, a read of standard input, a strand parking on a
// socket or a timer — so that nothing a program said is held back while it
// stands still. Two million `println`s were four million `write` calls and
// fifteen times Go's buffered writer.

const OUT_CAP: usize = 64 * 1024;
static mut OUT_BUF: [u8; OUT_CAP] = [0; OUT_CAP];
static mut OUT_LEN: usize = 0;
/// -1 until asked; 1 when standard output is a terminal (line-buffered).
static mut OUT_TTY: i8 = -1;

fn out(bytes: &[u8]) {
    unsafe {
        // Several workers share no buffer: they write as they did.
        if rill_mt != 0 {
            write_all(1, bytes);
            return;
        }
        if OUT_TTY < 0 {
            OUT_TTY = (isatty(1) != 0) as i8;
        }
        if bytes.len() >= OUT_CAP {
            rill_flush_out();
            write_all(1, bytes);
            return;
        }
        if OUT_LEN + bytes.len() > OUT_CAP {
            rill_flush_out();
        }
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), OUT_BUF.as_mut_ptr().add(OUT_LEN), bytes.len());
        OUT_LEN += bytes.len();
        if OUT_TTY == 1 && bytes.contains(&b'\n') {
            rill_flush_out();
        }
    }
}

unsafe fn write_all(fd: i32, bytes: &[u8]) {
    let mut done = 0usize;
    while done < bytes.len() {
        let n = write(fd, bytes.as_ptr().add(done), bytes.len() - done);
        if n <= 0 {
            return;
        }
        done += n as usize;
    }
}

/// Writes out whatever standard output is holding. The compiler calls it
/// when `main` returns; the runtime, before it waits.
///
/// # Safety
/// Single-threaded standard output; harmless when there is nothing held.
#[no_mangle]
pub unsafe extern "C" fn rill_flush_out() {
    if OUT_LEN > 0 {
        let n = OUT_LEN;
        OUT_LEN = 0;
        write_all(1, core::slice::from_raw_parts(OUT_BUF.as_ptr(), n));
    }
}

/// Registered with `atexit` when the runtime starts, so that whatever is
/// still held is written however the process ends — `main` returning, an
/// `exit`, or a C callback that ran after `main` and printed.
extern "C" fn flush_at_exit() {
    unsafe {
        rill_flush_out();
        #[cfg(not(any(target_os = "wasi", target_os = "none")))]
        net::flush_pending_at_exit();
    }
}

/// `abort`, with standard output written out first so the message that
/// explains the abort is not lost with it.
fn die() -> ! {
    unsafe {
        rill_flush_out();
        abort()
    }
}

// ---- strings (M5) ---------------------------------------------------------
//
// A string is a pointer to `{ i64 rc, i64 len, bytes... }`. Literals are
// compiler-emitted writable globals whose rc starts at IMMORTAL, so they are
// never freed (retains/releases on them are harmless counter churn).

const IMMORTAL: i64 = i64::MAX / 2;

unsafe fn str_len_of(s: *const u8) -> usize {
    *(s as *const i64).add(1) as usize
}

unsafe fn str_bytes(s: *const u8) -> *const u8 {
    s.add(16)
}

/// Allocates an uninitialized counted string of the given length (rc = 1).
unsafe fn str_new(len: usize) -> *mut u8 {
    let p = rill_alloc((16 + len) as u64);
    *(p as *mut i64) = 1;
    *(p as *mut i64).add(1) = len as i64;
    p
}

/// A string with the bytes of a `Buf(U8)`, copied.
///
/// # Safety
/// `v` must be a live byte buffer.
#[no_mangle]
pub unsafe extern "C" fn rill_str_from_buf(v: *const u8) -> *mut u8 {
    let n = *(v.sub(16) as *const i64).add(1);
    rill_str_from_ptr(v, n)
}

/// The same, but the block is taken over: a byte buffer's header — a count
/// and a length, sixteen bytes before the elements — is a string's header
/// with the same numbers in it, so a buffer that nothing will touch again
/// *is* the string, and `upper` over forty megabytes is one loop and no
/// copy. The compiler calls this in place of `rill_str_from_buf` when the
/// buffer is the frame's own and never named again; the frame's release of
/// the buffer still runs, so the block keeps one count for the string. A
/// buffer some other holder shares is copied after all.
///
/// # Safety
/// `v` must be a live byte buffer that the caller will not write again.
#[no_mangle]
pub unsafe extern "C" fn rill_str_take_buf(v: *const u8) -> *mut u8 {
    let block = v.sub(16) as *mut u8;
    if *(block as *const i64) != 1 {
        return rill_str_from_buf(v);
    }
    rc_inc(block as *mut i64);
    block
}

/// # Safety
/// `s` must be a live string object.
#[no_mangle]
pub unsafe extern "C" fn rill_println_str(s: *const u8) {
    out(core::slice::from_raw_parts(str_bytes(s), str_len_of(s)));
    out(b"\n");
}

/// # Safety
/// `s` must be a live string object.
#[no_mangle]
pub unsafe extern "C" fn rill_str_release(s: *mut u8) {
    let size = 16 + str_len_of(s);
    if rc_dec(s as *mut i64) == 0 {
        rill_free_sized(s, size as u64);
    }
}

/// # Safety
/// Both must be live string objects; returns a new owned string.
#[no_mangle]
pub unsafe extern "C" fn rill_str_concat(a: *const u8, b: *const u8) -> *mut u8 {
    let (la, lb) = (str_len_of(a), str_len_of(b));
    let p = str_new(la + lb);
    copy_small(str_bytes(a), p.add(16), la);
    copy_small(str_bytes(b), p.add(16 + la), lb);
    p
}

// ---- string builders -------------------------------------------------------
//
// A string is a counted value, so building one a piece at a time with `+`
// copies what is already there every time — quadratic in the length. A builder
// is what a `Buf` is, for bytes on their way to becoming a string:
// `{ i64 rc, *mut u8 data, i64 len, i64 cap }`. The bytes live behind a
// pointer rather than in the object, so growing them leaves the object where
// it is and the name that holds it stays good.

unsafe fn sb_data(p: *const u8) -> *mut u8 {
    *(p as *const *mut u8).add(1)
}

unsafe fn sb_len(p: *const u8) -> usize {
    *(p as *const i64).add(2) as usize
}

/// # Safety
/// Hands back a live builder, owned by the caller.
#[no_mangle]
pub unsafe extern "C" fn rill_strbuf_new() -> *mut u8 {
    let p = rill_alloc(32);
    *(p as *mut i64) = 1;
    *(p as *mut *mut u8).add(1) = core::ptr::null_mut();
    *(p as *mut i64).add(2) = 0;
    *(p as *mut i64).add(3) = 0;
    p
}

/// Copies `n` bytes with a loop when there are few of them. `memcpy` is a
/// call into another library that then works out how to go fast; for the two
/// or three bytes of `","` or `"null"` the working-out costs more than the
/// copy. Text is assembled a piece at a time, and the pieces are short.
///
/// # Safety
/// The ranges must not overlap and must both be `n` bytes long.
#[inline(always)]
unsafe fn copy_small(src: *const u8, dst: *mut u8, n: usize) {
    if n > 16 {
        core::ptr::copy_nonoverlapping(src, dst, n);
        return;
    }
    // Two overlapping words rather than a byte at a time: a fifteen-byte
    // line is two loads and two stores, not fifteen of each.
    if n >= 8 {
        let a = (src as *const u64).read_unaligned();
        let b = (src.add(n - 8) as *const u64).read_unaligned();
        (dst as *mut u64).write_unaligned(a);
        (dst.add(n - 8) as *mut u64).write_unaligned(b);
        return;
    }
    if n >= 4 {
        let a = (src as *const u32).read_unaligned();
        let b = (src.add(n - 4) as *const u32).read_unaligned();
        (dst as *mut u32).write_unaligned(a);
        (dst.add(n - 4) as *mut u32).write_unaligned(b);
        return;
    }
    let mut i = 0;
    while i < n {
        *dst.add(i) = *src.add(i);
        i += 1;
    }
}

/// Moves a builder's bytes into a larger block. The block comes from the same
/// pool as everything else rather than from `malloc`, because a program that
/// builds one short string per input token — a JSON parser, say — asks for
/// one of these per token, and the pool answers from a free list where
/// `malloc` walks its own bookkeeping.
///
/// # Safety
/// `p` must be a live builder holding `len` bytes in a block of `cap`.
/// A bigger block for a builder, and nothing else: the caller writes the new
/// pointer and the new capacity into the header itself. The compiler emits
/// the appends around this, and a call that touched the header would cost it
/// the length and the capacity it is holding in registers — which is most of
/// what an append costs in a loop.
///
/// # Safety
/// `data` is a block this builder owns, or null, with `len` bytes of text in
/// it and room for `cap`.
#[no_mangle]
pub unsafe extern "C" fn rill_sb_grow(
    data: *mut u8,
    len: i64,
    cap: i64,
    want: i64,
) -> *mut u8 {
    let fresh = rill_alloc(16 + want as u64).add(16);
    live_add(-1);
    if !data.is_null() {
        core::ptr::copy_nonoverlapping(data, fresh, len as usize);
        live_add(1);
        rill_free_sized(data.sub(16), 16 + cap as u64);
    }
    fresh
}

unsafe fn sb_grow(p: *mut u8, len: usize, cap: usize, want: usize) {
    // Sixteen bytes in front of the text are kept free, in the shape of a
    // string's header, so that `rill_strbuf_take` can turn the block into the
    // string itself instead of copying it out. `data` points at the text.
    let fresh = rill_alloc(16 + want as u64).add(16);
    // The block is raw bytes, not an object with a count: the allocation
    // ledger must not see it, or every live builder would look like a leak.
    live_add(-1);
    let old = sb_data(p);
    if !old.is_null() {
        core::ptr::copy_nonoverlapping(old, fresh, len);
        live_add(1);
        rill_free_sized(old.sub(16), 16 + cap as u64);
    }
    *(p as *mut *mut u8).add(1) = fresh;
    *(p as *mut i64).add(3) = want as i64;
}

/// Empties a builder without giving back the room it took, so that text
/// built over and over into the same builder pays for its room once.
///
/// # Safety
/// `p` must be a live builder.
#[no_mangle]
pub unsafe extern "C" fn rill_strbuf_clear(p: *mut u8) {
    *(p as *mut i64).add(2) = 0;
}

/// # Safety
/// `p` must be a live builder and `s` a live string.
#[no_mangle]
pub unsafe extern "C" fn rill_strbuf_add(p: *mut u8, s: *const u8) {
    let n = str_len_of(s);
    if n == 0 {
        return;
    }
    let len = sb_len(p);
    let cap = *(p as *const i64).add(3) as usize;
    if len + n > cap {
        let mut want = if cap == 0 { 32 } else { cap };
        while want < len + n {
            want *= 2;
        }
        sb_grow(p, len, cap, want);
    }
    copy_small(str_bytes(s), sb_data(p).add(len), n);
    *(p as *mut i64).add(2) = (len + n) as i64;
}

/// # Safety
/// `p` must be a live builder. Adds one byte, which is what reading a string
/// a character at a time produces — `chr` would allocate a string for each.
#[no_mangle]
pub unsafe extern "C" fn rill_strbuf_byte(p: *mut u8, c: i64) {
    let len = sb_len(p);
    let cap = *(p as *const i64).add(3) as usize;
    if len + 1 > cap {
        sb_grow(p, len, cap, if cap == 0 { 32 } else { cap * 2 });
    }
    *sb_data(p).add(len) = c as u8;
    *(p as *mut i64).add(2) = (len + 1) as i64;
}

/// Room for `n` bytes more before the next append has to grow the block:
/// `replace` knows its result is about the size of its input, and a builder
/// that starts there is not copied at every doubling on the way up.
///
/// # Safety
/// `p` must be a live builder.
#[no_mangle]
pub unsafe extern "C" fn rill_strbuf_reserve(p: *mut u8, n: i64) {
    let have = sb_len(p);
    let cap = *(p as *const i64).add(3) as usize;
    let want = have + n.max(0) as usize;
    if want > cap {
        sb_grow(p, have, cap, want);
    }
}

/// `b += s[start:start+len]` without the string in between: the bytes go
/// straight from `s` into the builder. `replace`, and anything else that
/// copies a text piece by piece, was allocating and freeing a string per
/// piece for what is one `memcpy`. The range is clamped as `str_sub` clamps
/// it.
///
/// # Safety
/// `p` must be a live builder and `s` a live string.
#[no_mangle]
pub unsafe extern "C" fn rill_strbuf_add_sub(p: *mut u8, s: *const u8, start: i64, len: i64) {
    let n = str_len_of(s) as i64;
    let a = start.clamp(0, n);
    let b = (a + len.max(0)).clamp(a, n);
    let m = (b - a) as usize;
    if m == 0 {
        return;
    }
    let have = sb_len(p);
    let cap = *(p as *const i64).add(3) as usize;
    if have + m > cap {
        let mut want = if cap == 0 { 32 } else { cap };
        while want < have + m {
            want *= 2;
        }
        sb_grow(p, have, cap, want);
    }
    copy_small(str_bytes(s).add(a as usize), sb_data(p).add(have), m);
    *(p as *mut i64).add(2) = (have + m) as i64;
}

/// `b += int_to_str(n)` without the string in between: the digits go straight
/// into the builder. Five million numbers through a fresh `Str` each cost 54
/// ms of allocating, copying and freeing; this is the copy alone.
///
/// # Safety
/// `p` must be a live builder.
#[no_mangle]
pub unsafe extern "C" fn rill_strbuf_int(p: *mut u8, v: i64) {
    let total = dec_len(v);
    let len = sb_len(p);
    let cap = *(p as *const i64).add(3) as usize;
    if len + total > cap {
        let mut want = if cap == 0 { 32 } else { cap };
        while want < len + total {
            want *= 2;
        }
        sb_grow(p, len, cap, want);
    }
    dec_into(v, sb_data(p).add(len), total);
    *(p as *mut i64).add(2) = (len + total) as i64;
}

/// # Safety
/// `p` must be a live builder; hands back a new owned string.
#[no_mangle]
pub unsafe extern "C" fn rill_strbuf_str(p: *const u8) -> *mut u8 {
    let len = sb_len(p);
    let out = str_new(len);
    if len > 0 {
        copy_small(sb_data(p), out.add(16), len);
    }
    out
}

/// `strbuf_str` for a builder nobody reads again: the block the text is in
/// becomes the string, header written in the room kept for it, and the
/// builder is left empty and usable. Only for a block beyond the pool
/// holding a string that is beyond it too — under that the copy is cheap
/// and the pool wants blocks back in the size class they came from, which
/// a string freed by its length would not do.
/// The compiler calls this where it can see the builder's last use.
///
/// # Safety
/// `p` must be a live builder.
#[no_mangle]
pub unsafe extern "C" fn rill_strbuf_take(p: *mut u8) -> *mut u8 {
    let len = sb_len(p);
    let cap = *(p as *const i64).add(3) as usize;
    let d = sb_data(p);
    // A string is freed by its length, a builder's block by its capacity,
    // and the two must land in the same allocator: a block beyond the
    // pool handed to a string short enough for the pool would be pushed
    // onto a free list it never came from, and stay there — 272 bytes of
    // `malloc` kept for every 200-byte answer a server ever gave.
    if d.is_null() || 16 + cap <= POOL_MAX || 16 + len <= POOL_MAX {
        return rill_strbuf_str(p);
    }
    let block = d.sub(16);
    *(block as *mut i64) = 1;
    *(block as *mut i64).add(1) = len as i64;
    // The block joins the ledger as a string; the builder's text is gone.
    live_add(1);
    *(p as *mut *mut u8).add(1) = core::ptr::null_mut();
    *(p as *mut i64).add(2) = 0;
    *(p as *mut i64).add(3) = 0;
    block
}

/// # Safety
/// `p` must be a live builder.
#[no_mangle]
pub unsafe extern "C" fn rill_strbuf_len(p: *const u8) -> i64 {
    sb_len(p) as i64
}

/// # Safety
/// `p` must be a live builder.
#[no_mangle]
pub unsafe extern "C" fn rill_strbuf_release(p: *mut u8) {
    if rc_dec(p as *mut i64) == 0 {
        let d = sb_data(p);
        if !d.is_null() {
            live_add(1);
            rill_free_sized(d.sub(16), 16 + *(p as *const i64).add(3) as u64);
        }
        rill_free_sized(p, 32);
    }
}

/// # Safety
/// `s` must be a live string object.
#[no_mangle]
pub unsafe extern "C" fn rill_str_len(s: *const u8) -> i64 {
    str_len_of(s) as i64
}

/// Writes `v` in decimal into the end of `buf` and returns the range holding
/// it. Digits come out backwards and are placed from the right, so the caller
/// gets a contiguous slice without a second pass to reverse them.
///
/// This exists because `snprintf("%lld")` is not cheap: it walks a format
/// string, consults the locale (taking a lock to do it), and runs the whole
/// of `vfprintf` to produce at most twenty digits. A program that prints a
/// million numbers spends real time there.
/// Two digits at a time from a table, and the digit count decided by a
/// ladder of comparisons rather than a division per digit: a number is
/// formatted in about half the divisions the obvious loop takes, and the
/// loop was what five million `int_to_str`s in a row were paying for.
const DIGITS2: &[u8; 200] = b"0001020304050607080910111213141516171819\
2021222324252627282930313233343536373839\
4041424344454647484950515253545556575859\
6061626364656667686970717273747576777879\
8081828384858687888990919293949596979899";

#[inline]
fn digit_count(u: u64) -> usize {
    if u < 10_000 {
        if u < 100 {
            if u < 10 { 1 } else { 2 }
        } else if u < 1_000 {
            3
        } else {
            4
        }
    } else if u < 100_000_000 {
        if u < 1_000_000 {
            if u < 100_000 { 5 } else { 6 }
        } else if u < 10_000_000 {
            7
        } else {
            8
        }
    } else if u < 1_000_000_000_000 {
        if u < 10_000_000_000 {
            if u < 1_000_000_000 { 9 } else { 10 }
        } else if u < 100_000_000_000 {
            11
        } else {
            12
        }
    } else if u < 10_000_000_000_000_000 {
        if u < 100_000_000_000_000 {
            if u < 10_000_000_000_000 { 13 } else { 14 }
        } else if u < 1_000_000_000_000_000 {
            15
        } else {
            16
        }
    } else if u < 1_000_000_000_000_000_000 {
        if u < 100_000_000_000_000_000 { 17 } else { 18 }
    } else if u < 10_000_000_000_000_000_000 {
        19
    } else {
        20
    }
}

/// Writes the decimal form of `v` into `out[..n]`, where `n` is what
/// `dec_len` said; `out` must have that much room.
///
/// # Safety
/// `out` must be writable for `dec_len(v)` bytes.
#[inline]
unsafe fn dec_into(v: i64, out: *mut u8, n: usize) {
    let mut u = v.unsigned_abs();
    let mut i = n;
    while u >= 100 {
        let pair = (u % 100) as usize * 2;
        u /= 100;
        i -= 2;
        *out.add(i) = DIGITS2[pair];
        *out.add(i + 1) = DIGITS2[pair + 1];
    }
    if u >= 10 {
        let pair = u as usize * 2;
        i -= 2;
        *out.add(i) = DIGITS2[pair];
        *out.add(i + 1) = DIGITS2[pair + 1];
    } else {
        i -= 1;
        *out.add(i) = b'0' + u as u8;
    }
    if v < 0 {
        *out = b'-';
    }
}

#[inline]
fn dec_len(v: i64) -> usize {
    digit_count(v.unsigned_abs()) + (v < 0) as usize
}

#[inline]
fn dec(v: i64, buf: &mut [u8; 24]) -> usize {
    let n = dec_len(v);
    unsafe { dec_into(v, buf.as_mut_ptr().add(24 - n), n) };
    24 - n
}

/// # Safety
/// Returns a new owned string.
#[no_mangle]
pub unsafe extern "C" fn rill_int_to_str(v: i64) -> *mut u8 {
    let n = dec_len(v);
    let p = str_new(n);
    dec_into(v, p.add(16), n);
    p
}

/// # Safety
/// Returns a new owned string.
#[no_mangle]
pub unsafe extern "C" fn rill_float_to_str(v: f64) -> *mut u8 {
    let mut buf = [0u8; 40];
    let n = snprintf(buf.as_mut_ptr(), 39, b"%g\0".as_ptr(), v);
    let n = n.clamp(0, 39) as usize;
    let p = str_new(n);
    copy_small(buf.as_ptr(), p.add(16), n);
    p
}

#[repr(C)]
struct StaticStr<const N: usize> {
    rc: i64,
    len: i64,
    bytes: [u8; N],
}

static mut TRUE_STR: StaticStr<4> = StaticStr { rc: IMMORTAL, len: 4, bytes: *b"true" };
static mut FALSE_STR: StaticStr<5> = StaticStr { rc: IMMORTAL, len: 5, bytes: *b"false" };

/// # Safety
/// Returns an immortal string.
#[no_mangle]
pub unsafe extern "C" fn rill_bool_to_str(v: i8) -> *mut u8 {
    if v != 0 {
        &raw mut TRUE_STR as *mut u8
    } else {
        &raw mut FALSE_STR as *mut u8
    }
}

// ---- string primitives (M7: text processing) ------------------------------

/// Byte at index, or -1 when out of range.
///
/// # Safety
/// `s` must be a live string object.
#[no_mangle]
pub unsafe extern "C" fn rill_str_get(s: *const u8, i: i64) -> i64 {
    if i < 0 || i as usize >= str_len_of(s) {
        return -1;
    }
    *str_bytes(s).add(i as usize) as i64
}

/// Substring by byte range, clamped to the string's bounds.
///
/// # Safety
/// `s` must be a live string object; returns a new owned string.
#[no_mangle]
pub unsafe extern "C" fn rill_str_sub(s: *const u8, start: i64, len: i64) -> *mut u8 {
    let n = str_len_of(s) as i64;
    let a = start.clamp(0, n);
    let b = (a + len.max(0)).clamp(a, n);
    let out = str_new((b - a) as usize);
    copy_small(str_bytes(s).add(a as usize), out.add(16), (b - a) as usize);
    out
}

/// Byte index of the first occurrence of `needle`, or -1. Empty needle -> 0.
///
/// # Safety
/// Both must be live string objects.
#[no_mangle]
pub unsafe extern "C" fn rill_str_find(s: *const u8, needle: *const u8) -> i64 {
    rill_str_find_from(s, needle, 0)
}

/// The same search, started partway along. Scanning a string without one of
/// these means slicing off the part already looked at, which copies it — so
/// walking a long line field by field cost more the further along it got.
///
/// A start before the beginning is clamped; one past the end finds nothing,
/// except for the empty needle, which is found wherever you look for it.
///
/// # Safety
/// Both must be live string objects.
#[no_mangle]
pub unsafe extern "C" fn rill_str_find_from(s: *const u8, needle: *const u8, from: i64) -> i64 {
    let (n, m) = (str_len_of(s), str_len_of(needle));
    let start = from.clamp(0, n as i64) as usize;
    if m == 0 {
        return start as i64;
    }
    if m + start > n {
        return -1;
    }
    find_bytes(str_bytes(s), n, str_bytes(needle), m, start)
}

/// Where `nee` (of `m >= 1` bytes) first begins in `hay[start..end]`, with
/// the match ending by `end`, or -1. `start + m <= end` is the caller's to
/// check.
///
/// One byte is `memchr`, which is the fastest thing there is for it. For
/// longer needles the haystack is crossed sixteen bytes at a time and a
/// position is looked at only when the needle's first byte *and* its last
/// byte are both where they would have to be: two rare things together are
/// far rarer than one, so the byte-by-byte compare runs almost only on real
/// matches. Jumping between first bytes with `memchr` and comparing at each
/// — the earlier way — spent its time in `memcmp` on text where the first
/// byte is common ("s" in "she sells sea shells"), and libc's `memmem` was
/// no better; this is 20% ahead of both on that text and level on text
/// where the first byte is rare.
#[cfg(target_arch = "aarch64")]
unsafe fn find_bytes(hay: *const u8, end: usize, nee: *const u8, m: usize, start: usize) -> i64 {
    use core::arch::aarch64::*;
    if m == 1 {
        let hit = memchr(hay.add(start), *nee as i32, end - start);
        return if hit.is_null() { -1 } else { (hit as usize - hay as usize) as i64 };
    }
    let stop = end - m;
    let first = vdupq_n_u8(*nee);
    let last = vdupq_n_u8(*nee.add(m - 1));
    let mut i = start;
    while i + 15 <= stop {
        let a = vld1q_u8(hay.add(i));
        let b = vld1q_u8(hay.add(i + m - 1));
        let both = vandq_u8(vceqq_u8(a, first), vceqq_u8(b, last));
        // Sixteen lanes of 0x00/0xFF to one bit per lane: narrow each pair
        // of lanes to a nibble and keep the top bit of every nibble.
        let narrowed = vshrn_n_u16::<4>(vreinterpretq_u16_u8(both));
        let mut mask = vget_lane_u64::<0>(vreinterpret_u64_u8(narrowed)) & 0x8888_8888_8888_8888;
        while mask != 0 {
            let at = i + (mask.trailing_zeros() / 4) as usize;
            if m == 2 || memcmp(hay.add(at + 1), nee.add(1), m - 2) == 0 {
                return at as i64;
            }
            mask &= mask - 1;
        }
        i += 16;
    }
    while i <= stop {
        if *hay.add(i) == *nee && *hay.add(i + m - 1) == *nee.add(m - 1) && memcmp(hay.add(i), nee, m) == 0 {
            return i as i64;
        }
        i += 1;
    }
    -1
}

/// The same search where there are no vector registers to write for:
/// `memchr` to each first byte, `memcmp` from there.
#[cfg(not(target_arch = "aarch64"))]
unsafe fn find_bytes(hay: *const u8, end: usize, nee: *const u8, m: usize, start: usize) -> i64 {
    let mut i = start;
    while i + m <= end {
        let hit = memchr(hay.add(i), *nee as i32, end - m - i + 1);
        if hit.is_null() {
            return -1;
        }
        i = hit as usize - hay as usize;
        if m == 1 || memcmp(hay.add(i), nee, m) == 0 {
            return i as i64;
        }
        i += 1;
    }
    -1
}

/// The same search again, but bounded: the first place `needle` begins at
/// or after `from` and ends at or before `to`, or -1. A search that must stay
/// inside a field of a line, or inside one chunk of a text that several
/// strands are scanning at once, wants this rather than a copy of the chunk
/// to search in — the copy costs as much as the search.
///
/// The bounds are clamped to the string; `to` below `from` finds nothing. The
/// empty needle is found at `from`, as `rill_str_find_from` finds it.
///
/// # Safety
/// Both must be live string objects.
#[no_mangle]
pub unsafe extern "C" fn rill_str_find_in(s: *const u8, needle: *const u8, from: i64, to: i64) -> i64 {
    let (n, m) = (str_len_of(s), str_len_of(needle));
    let start = from.clamp(0, n as i64) as usize;
    let end = to.clamp(start as i64, n as i64) as usize;
    if m == 0 {
        return start as i64;
    }
    if m + start > end {
        return -1;
    }
    find_bytes(str_bytes(s), end, str_bytes(needle), m, start)
}

/// Lexicographic byte comparison: -1, 0 or 1.
///
/// # Safety
/// Both must be live string objects.
#[no_mangle]
pub unsafe extern "C" fn rill_str_cmp(a: *const u8, b: *const u8) -> i64 {
    let x = core::slice::from_raw_parts(str_bytes(a), str_len_of(a));
    let y = core::slice::from_raw_parts(str_bytes(b), str_len_of(b));
    match x.cmp(y) {
        core::cmp::Ordering::Less => -1,
        core::cmp::Ordering::Equal => 0,
        core::cmp::Ordering::Greater => 1,
    }
}

/// Parses an optional `-` followed by digits; stops at the first non-digit.
/// Unparseable input yields 0.
///
/// # Safety
/// `s` must be a live string object.
#[no_mangle]
pub unsafe extern "C" fn rill_str_to_int(s: *const u8) -> i64 {
    let bytes = core::slice::from_raw_parts(str_bytes(s), str_len_of(s));
    let mut i = 0;
    let neg = if bytes.first() == Some(&b'-') {
        i = 1;
        true
    } else {
        false
    };
    let mut v: i64 = 0;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        v = v.wrapping_mul(10).wrapping_add((bytes[i] - b'0') as i64);
        i += 1;
    }
    if neg {
        -v
    } else {
        v
    }
}

/// Parses a leading decimal number; unparseable input yields 0.
///
/// # Safety
/// `s` must be a live string object.
#[no_mangle]
pub unsafe extern "C" fn rill_str_to_float(s: *const u8) -> f64 {
    let c = cstr(s);
    let v = strtod(c, core::ptr::null_mut());
    free(c);
    v
}

/// A one-byte string from a byte value (masked to 0..255).
#[no_mangle]
pub extern "C" fn rill_chr(c: i64) -> *mut u8 {
    unsafe {
        let p = str_new(1);
        *p.add(16) = (c & 0xff) as u8;
        p
    }
}

// ---- process and file IO (M7) ---------------------------------------------

static mut ARGC: i64 = 0;
static mut ARGV: *const *const u8 = core::ptr::null();
/// errno-style status of the most recent IO operation (single-threaded).
static mut IO_OK: i8 = 1;

/// # Safety
/// Called once from the generated `main` with the C argc/argv.
#[no_mangle]
pub unsafe extern "C" fn rill_set_args(argc: i64, argv: *const *const u8) {
    ARGC = argc;
    ARGV = argv;
}

#[no_mangle]
pub extern "C" fn rill_arg_count() -> i64 {
    unsafe { ARGC }
}

/// # Safety
/// Returns a new owned string; out-of-range index yields "".
#[no_mangle]
pub unsafe extern "C" fn rill_arg(i: i64) -> *mut u8 {
    if i < 0 || i >= ARGC {
        return str_new(0);
    }
    let c = *ARGV.add(i as usize);
    let mut n = 0;
    while *c.add(n) != 0 {
        n += 1;
    }
    let p = str_new(n);
    core::ptr::copy_nonoverlapping(c, p.add(16), n);
    p
}

#[no_mangle]
pub extern "C" fn rill_io_ok() -> i8 {
    unsafe { IO_OK }
}

/// Copies a Rill string into a NUL-terminated C buffer (caller frees).
unsafe fn cstr(s: *const u8) -> *mut u8 {
    let n = str_len_of(s);
    let p = malloc(n + 1);
    if p.is_null() {
        out(b"rill: out of memory\n");
        die();
    }
    core::ptr::copy_nonoverlapping(str_bytes(s), p, n);
    *p.add(n) = 0;
    p
}

/// Reads a whole file; on failure sets `io_ok` false and returns "".
///
/// # Safety
/// `path` must be a live string object; returns a new owned string.
#[no_mangle]
pub unsafe extern "C" fn rill_read_file(path: *const u8) -> *mut u8 {
    IO_OK = 1;
    let c = cstr(path);
    let fd = open(c, O_RDONLY);
    free(c);
    if fd < 0 {
        IO_OK = 0;
        return str_new(0);
    }
    let size = lseek(fd, 0, 2); // SEEK_END
    lseek(fd, 0, 0); // SEEK_SET
    if size < 0 {
        close(fd);
        IO_OK = 0;
        return str_new(0);
    }
    let p = str_new(size as usize);
    let mut got = 0usize;
    while got < size as usize {
        let r = read(fd, p.add(16 + got), size as usize - got);
        if r <= 0 {
            break;
        }
        got += r as usize;
    }
    close(fd);
    if got != size as usize {
        IO_OK = 0;
    }
    *(p as *mut i64).add(1) = got as i64;
    p
}

// The flags a file open needs, which are not the same everywhere. `O_CREAT`
// and `O_TRUNC` are 0x200 and 0x400 on the BSDs and 0x40 and 0x200 on Linux,
// and the numbers overlap rather than simply differing: the BSD pair asked for
// on Linux is `O_TRUNC | O_APPEND` with no `O_CREAT` in it at all. So writing
// over a file that exists worked and creating one did not, which is why this
// was wrong for as long as it was — nothing notices until the day a program
// wants a file that is not there yet. Written out one platform at a time, like
// the socket constants further down and for the same reason.
//
// WASI is a third set and the least like the others: its flags are the bits of
// a capability rather than a mode, so `O_RDONLY` is not zero there. Opening a
// file to read it with a flag word of 0 asks for a descriptor with no right to
// read, which is why file reading appeared to be missing from the wasm build
// rather than merely wrong.
#[cfg(not(any(target_os = "linux", target_os = "wasi")))]
const O_RDONLY: i32 = 0x0;
#[cfg(not(any(target_os = "linux", target_os = "wasi")))]
const O_WRONLY: i32 = 0x1;
#[cfg(not(any(target_os = "linux", target_os = "wasi")))]
const O_CREAT: i32 = 0x200;
#[cfg(not(any(target_os = "linux", target_os = "wasi")))]
const O_TRUNC: i32 = 0x400;

#[cfg(target_os = "linux")]
const O_RDONLY: i32 = 0o0;
#[cfg(target_os = "linux")]
const O_WRONLY: i32 = 0o1;
#[cfg(target_os = "linux")]
const O_CREAT: i32 = 0o100;
#[cfg(target_os = "linux")]
const O_TRUNC: i32 = 0o1000;

#[cfg(target_os = "wasi")]
const O_RDONLY: i32 = 0x04000000;
#[cfg(target_os = "wasi")]
const O_WRONLY: i32 = 0x10000000;
#[cfg(target_os = "wasi")]
const O_CREAT: i32 = 0x1 << 12;
#[cfg(target_os = "wasi")]
const O_TRUNC: i32 = 0x8 << 12;

/// Writes (creates/truncates) a whole file; returns success.
///
/// # Safety
/// Both must be live string objects.
#[no_mangle]
pub unsafe extern "C" fn rill_write_file(path: *const u8, content: *const u8) -> i8 {
    IO_OK = 1;
    let c = cstr(path);
    let fd = open(c, O_WRONLY | O_CREAT | O_TRUNC, 0o644 as i32);
    free(c);
    if fd < 0 {
        IO_OK = 0;
        return 0;
    }
    let n = str_len_of(content);
    let mut sent = 0usize;
    while sent < n {
        let w = write(fd, str_bytes(content).add(sent), n - sent);
        if w <= 0 {
            break;
        }
        sent += w as usize;
    }
    close(fd);
    if sent != n {
        IO_OK = 0;
        return 0;
    }
    1
}

// ---- files ----------------------------------------------------------------
//
// `read_file` and `write_file` are for a file a program has all of at once.
// What follows is for the other kind: a file too big to hold, or one being
// appended to for as long as the program runs — a log, a journal, a store
// whose readers are at different places in it.
//
// A file is a descriptor, the same `Int` a socket is, and for the same
// reason: the two families are then written alike, and a program that has
// both is not holding two different ideas of what an open thing is. The
// numbers do not overlap, because the operating system hands them out.
//
// Reads and writes go straight through. A read that the page cache answers
// costs a few microseconds — about what a channel rendezvous costs — and
// putting it on a helper thread would cost more than it saved. `file_sync` is
// the exception and the reason this section has threads in it at all: asking
// the disk to make what has been written durable takes milliseconds, and a
// worker standing still for that is a worker not running any of the strands
// that are ready. So a sync goes to a helper thread and the strand parks on a
// pipe, exactly as a strand waiting on a socket parks on the socket.

#[cfg(not(any(target_os = "linux", target_os = "wasi")))]
const O_RDWR: i32 = 0x2;
#[cfg(not(any(target_os = "linux", target_os = "wasi")))]
const O_APPEND: i32 = 0x8;

#[cfg(target_os = "linux")]
const O_RDWR: i32 = 0o2;
#[cfg(target_os = "linux")]
const O_APPEND: i32 = 0o2000;

// WASI's flags are the bits of a capability rather than a mode, so read-write
// is the two rights together; appending is an `fdflag` the shim folds in.
#[cfg(target_os = "wasi")]
const O_RDWR: i32 = O_RDONLY | O_WRONLY;
#[cfg(target_os = "wasi")]
const O_APPEND: i32 = 0x1;

#[cfg(unix)]
extern "C" {
    fn fsync(fd: i32) -> i32;
    fn ftruncate(fd: i32, len: i64) -> i32;
    fn pread(fd: i32, buf: *mut u8, count: usize, offset: i64) -> isize;
    fn pwrite(fd: i32, buf: *const u8, count: usize, offset: i64) -> isize;
    fn unlink(path: *const u8) -> i32;
    fn rename(from: *const u8, to: *const u8) -> i32;
    fn mkdir(path: *const u8, mode: u32) -> i32;
    fn access(path: *const u8, mode: i32) -> i32;
    fn pipe(fds: *mut i32) -> i32;
}

// Reading a directory is the one file operation whose shape is not the same on
// both Unixes. `struct dirent` differs in its first three fields, so the name
// is at a different offset in it; and on 64-bit Intel Macs the symbols
// themselves carry a suffix, because the struct changed there and the old one
// kept the plain name. Both are settled here rather than in the code that
// walks a directory.
#[cfg(all(unix, target_vendor = "apple"))]
const D_NAME_OFF: usize = 21;
#[cfg(all(unix, not(target_vendor = "apple")))]
const D_NAME_OFF: usize = 19;

#[cfg(unix)]
extern "C" {
    #[cfg_attr(all(target_vendor = "apple", target_arch = "x86_64"), link_name = "opendir$INODE64")]
    fn opendir(path: *const u8) -> *mut u8;
    #[cfg_attr(all(target_vendor = "apple", target_arch = "x86_64"), link_name = "readdir$INODE64")]
    fn readdir(dir: *mut u8) -> *mut u8;
    fn closedir(dir: *mut u8) -> i32;
}

// Everything a file call needs that is not a plain `open`/`read`/`write`, on
// the platforms that do not have the Unix spelling of it. Windows has all of
// them under other names; wasm has none and says so by failing.
#[cfg(windows)]
extern "C" {
    #[link_name = "_commit"]
    fn fsync(fd: i32) -> i32;
    #[link_name = "_chsize_s"]
    fn chsize_s(fd: i32, len: i64) -> i32;
    #[link_name = "_unlink"]
    fn unlink(path: *const u8) -> i32;
    fn rename(from: *const u8, to: *const u8) -> i32;
    #[link_name = "_mkdir"]
    fn mkdir_win(path: *const u8) -> i32;
    #[link_name = "_access"]
    fn access(path: *const u8, mode: i32) -> i32;
}

#[cfg(windows)]
unsafe fn ftruncate(fd: i32, len: i64) -> i32 {
    chsize_s(fd, len)
}

#[cfg(windows)]
unsafe fn mkdir(path: *const u8, _mode: u32) -> i32 {
    mkdir_win(path)
}

// Windows has no `pread`; seeking there and back is the same thing for a
// program that is not sharing the descriptor between threads, which a Rill
// program is not — a descriptor belongs to whoever opened it.
#[cfg(windows)]
unsafe fn pread(fd: i32, buf: *mut u8, count: usize, offset: i64) -> isize {
    let was = lseek(fd, 0, 1);
    if was < 0 || lseek(fd, offset, 0) < 0 {
        return -1;
    }
    let n = read(fd, buf, count);
    lseek(fd, was, 0);
    n
}

#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn fsync(_fd: i32) -> i32 {
    -1
}
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn ftruncate(_fd: i32, _len: i64) -> i32 {
    -1
}
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn pread(_fd: i32, _buf: *mut u8, _count: usize, _offset: i64) -> isize {
    -1
}
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn pwrite(_fd: i32, _buf: *const u8, _count: usize, _offset: i64) -> isize {
    -1
}
#[cfg(windows)]
unsafe fn pwrite(fd: i32, buf: *const u8, count: usize, offset: i64) -> isize {
    let was = lseek(fd, 0, 1);
    if was < 0 || lseek(fd, offset, 0) < 0 {
        return -1;
    }
    let n = write(fd, buf, count);
    lseek(fd, was, 0);
    n
}
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn unlink(_path: *const u8) -> i32 {
    -1
}
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn rename(_from: *const u8, _to: *const u8) -> i32 {
    -1
}
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn mkdir(_path: *const u8, _mode: u32) -> i32 {
    -1
}
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn access(_path: *const u8, _mode: i32) -> i32 {
    -1
}

/// The flags a mode string asks for, or -1 if it is not one.
///
/// The six C has, and no more: a mode is a thing a program writes once and
/// reads often, and a set of flags spelled out would be longer to read and no
/// more expressive. `"a"` is the one a log wants — every write goes to the end
/// of the file whatever else is writing to it, which is a property of the
/// descriptor rather than of remembering to seek.
unsafe fn open_flags(mode: *const u8) -> i32 {
    let n = str_len_of(mode);
    let b = str_bytes(mode);
    let plus = n == 2 && *b.add(1) == b'+';
    if n != 1 && !plus {
        return -1;
    }
    match *b {
        b'r' => {
            if plus {
                O_RDWR
            } else {
                O_RDONLY
            }
        }
        b'w' => (if plus { O_RDWR } else { O_WRONLY }) | O_CREAT | O_TRUNC,
        b'a' => (if plus { O_RDWR } else { O_WRONLY }) | O_CREAT | O_APPEND,
        _ => -1,
    }
}

/// Opens a file and returns its descriptor, or -1.
///
/// # Safety
/// Both must be live string objects.
#[no_mangle]
pub unsafe extern "C" fn rill_file_open(path: *const u8, mode: *const u8) -> i64 {
    IO_OK = 1;
    let flags = open_flags(mode);
    if flags < 0 {
        IO_OK = 0;
        return -1;
    }
    let c = cstr(path);
    let fd = open(c, flags, 0o644 as i32);
    free(c);
    if fd < 0 {
        IO_OK = 0;
        return -1;
    }
    fd as i64
}

/// Reads up to `n` bytes from where the descriptor is; `""` at the end.
///
/// # Safety
/// Returns a new owned string.
#[no_mangle]
pub unsafe extern "C" fn rill_file_read(fd: i64, n: i64) -> *mut u8 {
    IO_OK = 1;
    if n <= 0 {
        return str_new(0);
    }
    let p = str_new(n as usize);
    let got = read(fd as i32, p.add(16), n as usize);
    if got < 0 {
        IO_OK = 0;
        *(p as *mut i64).add(1) = 0;
        return p;
    }
    *(p as *mut i64).add(1) = got as i64;
    p
}

/// Reads up to `n` bytes from `off`, without moving the descriptor.
///
/// This is what lets one open file answer several readers at once — a
/// consumer reading a log from where it left off does not disturb the
/// producer appending to it, and two consumers at different places do not
/// disturb each other.
///
/// # Safety
/// Returns a new owned string.
#[no_mangle]
pub unsafe extern "C" fn rill_file_pread(fd: i64, off: i64, n: i64) -> *mut u8 {
    IO_OK = 1;
    if n <= 0 || off < 0 {
        return str_new(0);
    }
    let p = str_new(n as usize);
    let got = pread(fd as i32, p.add(16), n as usize, off);
    if got < 0 {
        IO_OK = 0;
        *(p as *mut i64).add(1) = 0;
        return p;
    }
    *(p as *mut i64).add(1) = got as i64;
    p
}

/// Reads at an offset into `n` bytes of a byte buffer the caller owns,
/// starting `at` bytes into it, and answers how many arrived (short at the
/// end of the file), or -1. `rill_file_pread` makes a string of what it
/// reads; a page cache, which reads the same-sized block into a slot of
/// memory it has already set aside, wants no string and no copy.
///
/// # Safety
/// `buf` must be a live byte buffer; the range is clamped to its length.
#[no_mangle]
pub unsafe extern "C" fn rill_file_pread_into(fd: i64, off: i64, buf: *mut u8, at: i64, n: i64) -> i64 {
    IO_OK = 1;
    let cap = *(buf.sub(16) as *const i64).add(1);
    let at = at.clamp(0, cap);
    let n = n.clamp(0, cap - at);
    let buf = buf.add(at as usize);
    if off < 0 {
        IO_OK = 0;
        return -1;
    }
    let mut got = 0usize;
    while (got as i64) < n {
        let r = pread(fd as i32, buf.add(got), (n as usize) - got, off + got as i64);
        if r < 0 {
            IO_OK = 0;
            return -1;
        }
        if r == 0 {
            break;
        }
        got += r as usize;
    }
    got as i64
}

/// Writes `n` bytes of a byte buffer, from `at` bytes into it, at an offset
/// in the file, without moving the descriptor — so that several strands may
/// write different pages of one file through it. All of it, or -1.
///
/// # Safety
/// `buf` must be a live byte buffer; the range is clamped to its length.
#[no_mangle]
pub unsafe extern "C" fn rill_file_pwrite(fd: i64, off: i64, buf: *const u8, at: i64, n: i64) -> i64 {
    IO_OK = 1;
    let cap = *(buf.sub(16) as *const i64).add(1);
    let at = at.clamp(0, cap);
    let n = n.clamp(0, cap - at) as usize;
    let buf = buf.add(at as usize);
    if off < 0 {
        IO_OK = 0;
        return -1;
    }
    let mut sent = 0usize;
    while sent < n {
        let w = pwrite(fd as i32, buf.add(sent), n - sent, off + sent as i64);
        if w <= 0 {
            IO_OK = 0;
            return -1;
        }
        sent += w as usize;
    }
    sent as i64
}

/// Takes the advisory lock on an open file — shared, or exclusive — and
/// answers whether it got it, at once: a lock someone else holds is `false`
/// rather than a wait, because a wait here would hold the worker and every
/// strand on it. A store that must not be opened twice takes the exclusive
/// lock when it opens and keeps it until it closes; the lock goes with the
/// descriptor. `flock`, so it is the whole file and it is advisory: it stops
/// other programs that ask, not ones that do not.
#[no_mangle]
pub unsafe extern "C" fn rill_file_lock(fd: i64, exclusive: i8) -> i8 {
    IO_OK = 1;
    let op = if exclusive != 0 { LOCK_EX } else { LOCK_SH } | LOCK_NB;
    if flock(fd as i32, op) != 0 {
        IO_OK = 0;
        return 0;
    }
    1
}

/// Lets the lock go before the file is closed.
#[no_mangle]
pub unsafe extern "C" fn rill_file_unlock(fd: i64) -> i8 {
    IO_OK = 1;
    if flock(fd as i32, LOCK_UN) != 0 {
        IO_OK = 0;
        return 0;
    }
    1
}

const LOCK_SH: i32 = 1;
const LOCK_EX: i32 = 2;
const LOCK_NB: i32 = 4;
const LOCK_UN: i32 = 8;

#[cfg(unix)]
extern "C" {
    fn flock(fd: i32, op: i32) -> i32;
}
#[cfg(not(unix))]
unsafe fn flock(_fd: i32, _op: i32) -> i32 {
    0
}

/// Writes all of a string; how many bytes went, or -1.
///
/// # Safety
/// `s` must be a live string object.
#[no_mangle]
pub unsafe extern "C" fn rill_file_write(fd: i64, s: *const u8) -> i64 {
    IO_OK = 1;
    let n = str_len_of(s);
    let mut sent = 0usize;
    while sent < n {
        let w = write(fd as i32, str_bytes(s).add(sent), n - sent);
        if w <= 0 {
            IO_OK = 0;
            return if sent == 0 { -1 } else { sent as i64 };
        }
        sent += w as usize;
    }
    sent as i64
}

/// Moves the descriptor to an absolute offset; the new offset, or -1.
#[no_mangle]
pub unsafe extern "C" fn rill_file_seek(fd: i64, off: i64) -> i64 {
    IO_OK = 1;
    let r = lseek(fd as i32, off, 0); // SEEK_SET
    if r < 0 {
        IO_OK = 0;
        return -1;
    }
    r
}

/// Where the descriptor is now, or -1.
#[no_mangle]
pub unsafe extern "C" fn rill_file_pos(fd: i64) -> i64 {
    IO_OK = 1;
    let r = lseek(fd as i32, 0, 1); // SEEK_CUR
    if r < 0 {
        IO_OK = 0;
        return -1;
    }
    r
}

/// How many bytes the file holds, or -1. The descriptor is left where it was.
#[no_mangle]
pub unsafe extern "C" fn rill_file_size(fd: i64) -> i64 {
    IO_OK = 1;
    let was = lseek(fd as i32, 0, 1); // SEEK_CUR
    let end = lseek(fd as i32, 0, 2); // SEEK_END
    if was >= 0 {
        lseek(fd as i32, was, 0);
    }
    if end < 0 {
        IO_OK = 0;
        return -1;
    }
    end
}

/// Cuts the file to `n` bytes, or grows it with zeros.
#[no_mangle]
pub unsafe extern "C" fn rill_file_truncate(fd: i64, n: i64) -> i8 {
    IO_OK = 1;
    if n < 0 || ftruncate(fd as i32, n) != 0 {
        IO_OK = 0;
        return 0;
    }
    1
}

/// Closes a descriptor.
#[no_mangle]
pub unsafe extern "C" fn rill_file_close(fd: i64) {
    close(fd as i32);
}

/// Removes a name from the filesystem.
///
/// # Safety
/// `path` must be a live string object.
#[no_mangle]
pub unsafe extern "C" fn rill_file_remove(path: *const u8) -> i8 {
    IO_OK = 1;
    let c = cstr(path);
    let r = unlink(c);
    free(c);
    if r != 0 {
        IO_OK = 0;
        return 0;
    }
    1
}

/// Renames a file, replacing whatever was at the new name.
///
/// On a Unix filesystem this is atomic: a reader sees the old file or the new
/// one and never half of either, which is what makes "write a temporary file,
/// sync it, rename it over the real one" the way to replace a file without a
/// window in which it is neither.
///
/// # Safety
/// Both must be live string objects.
#[no_mangle]
pub unsafe extern "C" fn rill_file_rename(from: *const u8, to: *const u8) -> i8 {
    IO_OK = 1;
    let a = cstr(from);
    let b = cstr(to);
    let r = rename(a, b);
    free(a);
    free(b);
    if r != 0 {
        IO_OK = 0;
        return 0;
    }
    1
}

/// Whether there is anything at that path.
///
/// # Safety
/// `path` must be a live string object.
#[no_mangle]
pub unsafe extern "C" fn rill_file_exists(path: *const u8) -> i8 {
    let c = cstr(path);
    let r = access(c, 0); // F_OK
    free(c);
    (r == 0) as i8
}

/// Makes a directory. Says false if it is already there.
///
/// # Safety
/// `path` must be a live string object.
#[no_mangle]
pub unsafe extern "C" fn rill_dir_make(path: *const u8) -> i8 {
    IO_OK = 1;
    let c = cstr(path);
    let r = mkdir(c, 0o755);
    free(c);
    if r != 0 {
        IO_OK = 0;
        return 0;
    }
    1
}

/// The names in a directory, one to a line, without `.` and `..`.
///
/// A line, because that is what the prelude already takes apart: `lines` and
/// `split` are how a Rill program reads a list of anything. A name may in
/// principle contain a newline and would then be two lines here; nothing that
/// writes a file on purpose does that, and the alternative — a length in front
/// of every name — would make the common case unreadable to pay for it.
///
/// # Safety
/// `path` must be a live string object; returns a new owned string.
#[no_mangle]
pub unsafe extern "C" fn rill_dir_list(path: *const u8) -> *mut u8 {
    IO_OK = 1;
    #[cfg(unix)]
    {
        let c = cstr(path);
        let d = opendir(c);
        free(c);
        if d.is_null() {
            IO_OK = 0;
            return str_new(0);
        }
        // Two passes over the directory would mean reading it twice and
        // trusting it not to change in between, so the names are gathered into
        // a growing buffer and copied into a string once at the end.
        let mut cap = 1024usize;
        let mut len = 0usize;
        let mut buf = malloc(cap);
        if buf.is_null() {
            closedir(d);
            IO_OK = 0;
            return str_new(0);
        }
        loop {
            let e = readdir(d);
            if e.is_null() {
                break;
            }
            let name = e.add(D_NAME_OFF);
            // `.` and `..` are in every directory and are never what a caller
            // asking what is in one means.
            if *name == b'.' && (*name.add(1) == 0 || (*name.add(1) == b'.' && *name.add(2) == 0)) {
                continue;
            }
            let mut n = 0usize;
            while *name.add(n) != 0 {
                n += 1;
            }
            if len + n + 1 > cap {
                while len + n + 1 > cap {
                    cap *= 2;
                }
                let bigger = malloc(cap);
                if bigger.is_null() {
                    free(buf);
                    closedir(d);
                    IO_OK = 0;
                    return str_new(0);
                }
                core::ptr::copy_nonoverlapping(buf, bigger, len);
                free(buf);
                buf = bigger;
            }
            core::ptr::copy_nonoverlapping(name, buf.add(len), n);
            len += n;
            *buf.add(len) = b'\n';
            len += 1;
        }
        closedir(d);
        let p = str_new(len);
        core::ptr::copy_nonoverlapping(buf, p.add(16), len);
        free(buf);
        p
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        IO_OK = 0;
        str_new(0)
    }
}

// -- making it durable --------------------------------------------------------
//
// A write that has returned is in the operating system's cache and not on the
// disk; a machine that loses power between the two loses it. `file_sync` is
// the wait for the disk to say it has the bytes, and it is the one file
// operation slow enough — a millisecond on a good drive, ten on a bad one —
// that a worker must not stand still for it.
//
// So it goes to a helper thread. The strand hands over a request and parks on
// a pipe, which is the same manoeuvre as parking on a socket and goes through
// the same poller: the worker is free the moment the request is handed over,
// and the strand becomes runnable again when the byte arrives. The overhead is
// four small syscalls, against a wait a thousand times longer.
//
// The request itself is on the heap and not on the strand's stack. A parked
// strand's frames are copied away while it waits — that is how one worker runs
// many strands on one stack — so an address into them means nothing to another
// thread. This is the one rule the whole mechanism rests on.

/// How many syncs may be in flight at once. Each is a thread doing nothing but
/// waiting on a disk, so they are cheap to have and there is no reason for
/// many: a handful of files being synced at once is already more than a log
/// does.
#[cfg(unix)]
const SYNC_HELPERS: usize = 4;

/// A pooled pipe. Making one is two syscalls, and a program that syncs in a
/// loop would make and destroy the same pair forever; they are kept instead.
#[cfg(unix)]
const POOLED_PIPES: usize = 64;

#[cfg(unix)]
#[repr(C)]
struct SyncReq {
    /// The descriptor to sync, or -1 when `path` names a directory instead.
    fd: i32,
    /// Where the helper says how it went.
    result: i32,
    /// The write end of the pipe the waiting strand is parked on.
    notify: i32,
    /// A directory to open, sync and close, or null.
    path: *mut u8,
}

#[cfg(unix)]
static mut SYNC_LOCK: PthreadMutex = PthreadMutex { sig: 0, opaque: [0; 56] };
#[cfg(unix)]
static mut SYNC_REQ_R: i32 = -1;
#[cfg(unix)]
static mut SYNC_REQ_W: i32 = -1;
#[cfg(unix)]
static mut SYNC_HELPERS_UP: i8 = 0;
#[cfg(unix)]
static mut PIPE_POOL: [[i32; 2]; POOLED_PIPES] = [[-1, -1]; POOLED_PIPES];
#[cfg(unix)]
static mut PIPE_POOLED: usize = 0;

/// Initialised before any strand exists, from `rill_rt_run`, so that the lock
/// below is usable without a race to create it. A program that never syncs
/// pays one `pthread_mutex_init` for this and nothing else: the threads and
/// the pipes are made on the first sync and not before.
#[cfg(unix)]
unsafe fn sync_init() {
    pthread_mutex_init(&raw mut SYNC_LOCK, core::ptr::null());
}

#[cfg(not(unix))]
unsafe fn sync_init() {}

/// The sync itself, wherever it is being run.
///
/// On a Mac `fsync` is not the whole story: it hands the bytes to the drive
/// and returns without waiting for the drive to put them on the platter, which
/// is fast and is not durability. `F_FULLFSYNC` is the call that waits, and it
/// is what a database on macOS uses. It is not supported on every filesystem,
/// so a failure falls back rather than being reported as one.
#[cfg(unix)]
unsafe fn do_sync(fd: i32, path: *mut u8) -> i32 {
    if !path.is_null() {
        let d = open(path, O_RDONLY, 0);
        if d < 0 {
            return -1;
        }
        let r = fsync(d);
        close(d);
        return r;
    }
    #[cfg(target_vendor = "apple")]
    {
        const F_FULLFSYNC: i32 = 51;
        extern "C" {
            fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        }
        if fcntl(fd, F_FULLFSYNC, 0) == 0 {
            return 0;
        }
    }
    fsync(fd)
}

#[cfg(unix)]
extern "C" fn sync_helper(_arg: *mut u8) -> *mut u8 {
    unsafe {
        loop {
            let mut req: *mut SyncReq = core::ptr::null_mut();
            // Every write to this pipe is exactly eight bytes and a pipe write
            // that small is atomic, so what is in the pipe is always a whole
            // number of requests and a reader asking for eight gets one of
            // them — even with several helpers reading the same pipe.
            let got = read(SYNC_REQ_R, &mut req as *mut *mut SyncReq as *mut u8, 8);
            if got != 8 {
                return core::ptr::null_mut();
            }
            (*req).result = do_sync((*req).fd, (*req).path);
            let done = [1u8];
            write((*req).notify, done.as_ptr(), 1);
        }
    }
}

/// Makes the request pipe and the helpers, once. Says whether they are there.
#[cfg(unix)]
unsafe fn sync_helpers_ready() -> bool {
    if SYNC_HELPERS_UP == 1 {
        return true;
    }
    if SYNC_HELPERS_UP == -1 {
        return false;
    }
    let mut fds = [-1i32; 2];
    if pipe(fds.as_mut_ptr()) != 0 {
        SYNC_HELPERS_UP = -1;
        return false;
    }
    SYNC_REQ_R = fds[0];
    SYNC_REQ_W = fds[1];
    let mut made = 0;
    for _ in 0..SYNC_HELPERS {
        let mut tid: usize = 0;
        if pthread_create(&mut tid, core::ptr::null(), sync_helper, core::ptr::null_mut()) == 0 {
            made += 1;
        }
    }
    if made == 0 {
        close(SYNC_REQ_R);
        close(SYNC_REQ_W);
        SYNC_REQ_R = -1;
        SYNC_REQ_W = -1;
        SYNC_HELPERS_UP = -1;
        return false;
    }
    SYNC_HELPERS_UP = 1;
    true
}

#[cfg(unix)]
unsafe fn take_pipe() -> [i32; 2] {
    if PIPE_POOLED > 0 {
        PIPE_POOLED -= 1;
        return PIPE_POOL[PIPE_POOLED];
    }
    let mut fds = [-1i32; 2];
    if pipe(fds.as_mut_ptr()) != 0 {
        return [-1, -1];
    }
    fds
}

#[cfg(unix)]
unsafe fn give_back_pipe(fds: [i32; 2]) {
    if PIPE_POOLED < POOLED_PIPES {
        PIPE_POOL[PIPE_POOLED] = fds;
        PIPE_POOLED += 1;
        return;
    }
    net::forget_pipe(fds[0]);
    close(fds[0]);
    close(fds[1]);
}

/// Hands a sync to a helper thread and parks until it answers.
///
/// Falls back to doing it here whenever it cannot be handed over — before the
/// scheduler exists, or if a thread or a pipe could not be made. The answer is
/// the same either way; what is lost is only that the worker waits.
#[cfg(unix)]
unsafe fn sync_off_worker(fd: i32, path: *mut u8) -> i32 {
    // No scheduler means no strand to park and no worker to spare. A program
    // that neither spawns nor waits runs `main` on the process stack, and
    // there is nothing for this to be an improvement on.
    if MAIN_G.is_null() {
        return do_sync(fd, path);
    }
    pthread_mutex_lock(&raw mut SYNC_LOCK);
    let up = sync_helpers_ready();
    let fds = if up { take_pipe() } else { [-1, -1] };
    let req_w = SYNC_REQ_W;
    pthread_mutex_unlock(&raw mut SYNC_LOCK);
    if !up || fds[0] < 0 {
        return do_sync(fd, path);
    }
    let req = malloc(core::mem::size_of::<SyncReq>()) as *mut SyncReq;
    if req.is_null() {
        pthread_mutex_lock(&raw mut SYNC_LOCK);
        give_back_pipe(fds);
        pthread_mutex_unlock(&raw mut SYNC_LOCK);
        return do_sync(fd, path);
    }
    (*req).fd = fd;
    (*req).result = -1;
    (*req).notify = fds[1];
    (*req).path = path;
    let handed = write(req_w, &req as *const *mut SyncReq as *const u8, 8);
    if handed != 8 {
        free(req as *mut u8);
        pthread_mutex_lock(&raw mut SYNC_LOCK);
        give_back_pipe(fds);
        pthread_mutex_unlock(&raw mut SYNC_LOCK);
        return do_sync(fd, path);
    }
    // From here the request belongs to the helper, and this strand's own
    // frames are about to be copied elsewhere. Nothing below may read anything
    // the helper is writing until the byte has arrived.
    net::park_on_pipe(fds[0]);
    let mut done = [0u8; 1];
    // Whether the poller woke this strand or the arm failed and it did not
    // park at all, the byte is what says the sync has happened; the pipe is
    // blocking, so this is where an unarmed wait ends up waiting. A read of a
    // pipe this end owns fails only for having been interrupted by a signal,
    // which is worth retrying and is not worth retrying forever.
    let mut tries = 0;
    while read(fds[0], done.as_mut_ptr(), 1) < 0 {
        tries += 1;
        if tries > 64 {
            break;
        }
    }
    let r = (*req).result;
    free(req as *mut u8);
    pthread_mutex_lock(&raw mut SYNC_LOCK);
    give_back_pipe(fds);
    pthread_mutex_unlock(&raw mut SYNC_LOCK);
    r
}

#[cfg(not(unix))]
unsafe fn sync_off_worker(fd: i32, path: *mut u8) -> i32 {
    if !path.is_null() {
        return -1;
    }
    fsync(fd)
}

/// Waits until everything written to the descriptor is on the disk.
#[no_mangle]
pub unsafe extern "C" fn rill_file_sync(fd: i64) -> i8 {
    IO_OK = 1;
    if sync_off_worker(fd as i32, core::ptr::null_mut()) != 0 {
        IO_OK = 0;
        return 0;
    }
    1
}

/// Waits until the directory's own contents — which names are in it — are on
/// the disk.
///
/// Syncing a file says the bytes are there. It does not say the *name* is:
/// creating a file and renaming one both change the directory, and that change
/// is cached like any other. A program that creates a file, syncs it, and
/// loses power can come back to a directory that has never heard of it. This
/// is the call that closes that gap, and the reason it is separate is that a
/// program pays for it only where it matters — after a create or a rename, not
/// after every write.
///
/// # Safety
/// `path` must be a live string object.
#[no_mangle]
pub unsafe extern "C" fn rill_dir_sync(path: *const u8) -> i8 {
    IO_OK = 1;
    let c = cstr(path);
    let r = sync_off_worker(-1, c);
    free(c);
    if r != 0 {
        IO_OK = 0;
        return 0;
    }
    1
}

// ---- signals --------------------------------------------------------------
//
// A server is told to stop by a signal, and a signal arrives in the worst
// context there is: on whichever thread the operating system picked, between
// any two instructions, possibly while a strand holds the channel lock.
// Nothing a Rill program has written may run there. What a handler may do is
// write a byte to a pipe, and that is all this one does.
//
// The strand does the rest. It parks on the pipe through the poller — the same
// manoeuvre a strand waiting on a socket makes — so a program waiting to be
// told to stop costs a worker nothing while it waits, which matters because
// waiting is the whole of what it does.
//
// One strand waits and tells the others, which is what `close` is for:
//
//     fn shutdown(done) =
//       wait_signal()
//       close(done)
//
// Several strands may still wait at once, each on a pipe of its own, because
// two strands parked on one descriptor would be two registrations of the same
// descriptor in the poller and only the second would be woken.

/// The three a server is told things with, and the three whose numbers every
/// Unix agrees on. `SIGUSR1` and `SIGUSR2` are not here because they are 30
/// and 31 on a Mac and 10 and 12 on Linux, and a number that means one thing
/// on the machine it was written on and another on the machine it runs on is
/// worse than not having it. `SIGHUP` is the traditional "read your
/// configuration again" and covers what a server would have used them for.
#[cfg(unix)]
const SIGNALS_HANDLED: [i32; 3] = [1 /* HUP */, 2 /* INT */, 15 /* TERM */];

/// How many strands may wait on a signal at once.
#[cfg(unix)]
const SIG_SLOTS: usize = 16;

#[cfg(unix)]
const SLOT_FREE: i32 = -1;

#[cfg(unix)]
extern "C" {
    fn signal(sig: i32, handler: usize) -> usize;
}

/// How many signals have arrived, and which the last one was. The pipe is
/// only the wakeup; the answer is here.
///
/// Counting rather than queueing is what makes a byte left in a pipe harmless.
/// A waiter notes the count before it parks and compares afterwards, so a
/// wakeup that is not a new signal simply parks again, and no bookkeeping has
/// to be right for that to be true.
#[cfg(unix)]
static SIG_COUNT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(unix)]
static SIG_LAST: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(0);

/// The write end of each waiting strand's pipe, or -1 for a free slot.
#[cfg(unix)]
static SIG_SLOT: [core::sync::atomic::AtomicI32; SIG_SLOTS] =
    [const { core::sync::atomic::AtomicI32::new(SLOT_FREE) }; SIG_SLOTS];

/// Both ends of each slot's pipe. Made once and never closed: a descriptor
/// this handler might write to must not be a number the operating system can
/// hand to something else, and a handler cannot take a lock to be sure of it.
#[cfg(unix)]
static mut SIG_PIPE: [[i32; 2]; SIG_SLOTS] = [[-1, -1]; SIG_SLOTS];
#[cfg(unix)]
static mut SIG_INSTALLED: i8 = 0;

/// Everything that happens in signal context, and nothing else may be added to
/// it: two atomic stores and a write of one byte per waiting strand. No
/// allocation, no lock, nothing that could already be held by the thread this
/// interrupted.
#[cfg(unix)]
extern "C" fn signal_landed(sig: i32) {
    unsafe {
        SIG_LAST.store(sig, core::sync::atomic::Ordering::Release);
        SIG_COUNT.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        let byte = [1u8];
        for slot in SIG_SLOT.iter() {
            let fd = slot.load(core::sync::atomic::Ordering::Acquire);
            if fd >= 0 {
                write(fd, byte.as_ptr(), 1);
            }
        }
    }
}

/// Installs the handlers, once, and makes the pipes. Called with `SYNC_LOCK`
/// held, which is what makes "once" true.
///
/// Until this runs a program behaves as it always did: a signal it has never
/// asked about does what the operating system does with it, so a program that
/// never calls `wait_signal` still dies on an interrupt. After it runs, it
/// does not — which is the point, and is also the thing to remember about a
/// program that waits for a signal and then does nothing with it.
#[cfg(unix)]
unsafe fn signals_ready() -> bool {
    if SIG_INSTALLED == 1 {
        return true;
    }
    if SIG_INSTALLED == -1 {
        return false;
    }
    for i in 0..SIG_SLOTS {
        let mut fds = [-1i32; 2];
        if pipe(fds.as_mut_ptr()) != 0 {
            SIG_INSTALLED = -1;
            return false;
        }
        SIG_PIPE[i] = fds;
    }
    for sig in SIGNALS_HANDLED {
        signal(sig, signal_landed as usize);
    }
    SIG_INSTALLED = 1;
    true
}

/// Parks until one of the handled signals arrives, and answers which:
/// 1 for `SIGHUP`, 2 for `SIGINT`, 15 for `SIGTERM`.
///
/// # Safety
/// Called from strand context.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn rill_wait_signal() -> i64 {
    pthread_mutex_lock(&raw mut SYNC_LOCK);
    let up = signals_ready();
    let mut mine = usize::MAX;
    if up {
        for (i, slot) in SIG_SLOT.iter().enumerate() {
            if slot.load(core::sync::atomic::Ordering::Acquire) == SLOT_FREE {
                slot.store(SIG_PIPE[i][1], core::sync::atomic::Ordering::Release);
                mine = i;
                break;
            }
        }
    }
    pthread_mutex_unlock(&raw mut SYNC_LOCK);
    if !up {
        out(b"rill: cannot listen for signals\n");
        die();
    }
    if mine == usize::MAX {
        out(b"rill: too many strands waiting on a signal - have one wait and close a channel the others watch\n");
        die();
    }
    let read_end = SIG_PIPE[mine][0];
    let seen = SIG_COUNT.load(core::sync::atomic::Ordering::Acquire);
    let mut byte = [0u8; 1];
    loop {
        if SIG_COUNT.load(core::sync::atomic::Ordering::Acquire) != seen {
            break;
        }
        // Before the scheduler exists there is one strand and nothing to hand
        // the worker to, so the read is simply the wait.
        if !MAIN_G.is_null() {
            net::park_on_pipe(read_end);
        }
        // A byte may be one this signal left, or one an earlier signal left in
        // a slot nobody was waiting on. The count above says which, so this
        // only has to take the byte out of the way.
        read(read_end, byte.as_mut_ptr(), 1);
    }
    SIG_SLOT[mine].store(SLOT_FREE, core::sync::atomic::Ordering::Release);
    SIG_LAST.load(core::sync::atomic::Ordering::Acquire) as i64
}

/// Nothing to listen to and nothing to listen with.
#[cfg(not(unix))]
#[no_mangle]
pub unsafe extern "C" fn rill_wait_signal() -> i64 {
    out(b"rill: signals are not available on this target\n");
    die()
}

// Standard input is read in blocks and handed out a line at a time. A
// syscall per byte is what this used to be, and on a large input that was all
// the program did; nothing else reads this descriptor, so holding what was
// read ahead is safe.
const IN_CAP: usize = 1 << 18;
static mut IN_BUF: *mut u8 = core::ptr::null_mut();
static mut IN_POS: usize = 0;
static mut IN_LEN: usize = 0;

/// The next byte of standard input, or -1 at the end of it.
unsafe fn in_byte() -> i32 {
    if IN_POS == IN_LEN {
        if IN_BUF.is_null() {
            IN_BUF = malloc(IN_CAP);
        }
        rill_flush_out();
        let got = read(0, IN_BUF, IN_CAP);
        if got <= 0 {
            return -1;
        }
        IN_LEN = got as usize;
        IN_POS = 0;
    }
    let b = *IN_BUF.add(IN_POS);
    IN_POS += 1;
    b as i32
}

/// Puts back the byte just taken, so a scanner can stop on the delimiter it
/// found without swallowing it.
unsafe fn in_unread() {
    if IN_POS > 0 {
        IN_POS -= 1;
    }
}

/// Reads the next whitespace-delimited integer from stdin. Nothing is
/// allocated on the way, which is the difference between this and splitting a
/// line into a list of strings and converting each. Sets `io_ok` false at the
/// end of the input or on anything that is not a number.
#[no_mangle]
pub unsafe extern "C" fn rill_scan_int() -> i64 {
    IO_OK = 1;
    let mut c = in_byte();
    while c == 32 || c == 10 || c == 9 || c == 13 {
        c = in_byte();
    }
    if c < 0 {
        IO_OK = 0;
        return 0;
    }
    let neg = c == b'-' as i32;
    if neg || c == b'+' as i32 {
        c = in_byte();
    }
    if !(48..=57).contains(&c) {
        IO_OK = 0;
        return 0;
    }
    let mut v: i64 = 0;
    while (48..=57).contains(&c) {
        v = v.wrapping_mul(10).wrapping_add((c - 48) as i64);
        c = in_byte();
    }
    if c >= 0 {
        in_unread();
    }
    if neg {
        -v
    } else {
        v
    }
}

/// Reads one line from stdin (newline stripped); EOF with no data sets
/// `io_ok` false.
///
/// # Safety
/// Returns a new owned string.
#[no_mangle]
pub unsafe extern "C" fn rill_read_line() -> *mut u8 {
    IO_OK = 1;
    // The line is usually already in the block read last time, whole: then
    // it is one `memchr` and one copy, straight into the string.
    if IN_POS < IN_LEN {
        let from = IN_BUF.add(IN_POS);
        let hit = memchr(from, b'\n' as i32, IN_LEN - IN_POS);
        if !hit.is_null() {
            let take = hit as usize - from as usize;
            let p = str_new(take);
            copy_small(from, p.add(16), take);
            IN_POS += take + 1;
            return p;
        }
    }
    let mut cap = 128usize;
    let mut buf = rill_alloc(cap as u64);
    live_add(-1);
    let mut n = 0usize;
    loop {
        if IN_POS == IN_LEN {
            if IN_BUF.is_null() {
                IN_BUF = malloc(IN_CAP);
            }
            rill_flush_out();
        let got = read(0, IN_BUF, IN_CAP);
            if got <= 0 {
                if n == 0 {
                    IO_OK = 0;
                }
                break;
            }
            IN_LEN = got as usize;
            IN_POS = 0;
        }
        // Whole runs at a time rather than a byte at a time: `memchr` finds
        // the newline sixteen bytes at a step, and what is before it is one
        // copy. A megabyte-long line used to be a megabyte of calls.
        let avail = IN_LEN - IN_POS;
        let from = IN_BUF.add(IN_POS);
        let hit = memchr(from, b'\n' as i32, avail);
        let take = if hit.is_null() { avail } else { hit as usize - from as usize };
        if n + take > cap {
            let mut want = cap;
            while want < n + take {
                want *= 2;
            }
            let bigger = rill_alloc(want as u64);
            live_add(-1);
            copy_small(buf, bigger, n);
            live_add(1);
            rill_free_sized(buf, cap as u64);
            buf = bigger;
            cap = want;
        }
        copy_small(from, buf.add(n), take);
        n += take;
        IN_POS += take;
        if !hit.is_null() {
            IN_POS += 1;
            break;
        }
    }
    let p = str_new(n);
    copy_small(buf, p.add(16), n);
    live_add(1);
    rill_free_sized(buf, cap as u64);
    p
}

/// # Safety
/// Terminates the process.
#[no_mangle]
pub unsafe extern "C" fn rill_exit(code: i64) -> ! {
    rill_flush_out();
    exit(code as i32)
}

// ---- time -----------------------------------------------------------------
//
// A monotonic clock, and nothing else. Not the wall clock: this is for
// measuring intervals — how long a tick took, how long until the next one —
// and a wall clock is the wrong instrument for that, because it can be set
// backwards under you by an administrator or a time server and hand you a
// negative duration.
//
// `CLOCK_MONOTONIC` is 1 on Linux and 6 on macOS. They are different numbers
// for the same idea, and getting them the wrong way round is not a compile
// error — it is a clock that quietly counts something else — so they are
// written out one platform at a time rather than shared.

#[cfg(all(unix, target_vendor = "apple"))]
const CLOCK_MONOTONIC: i32 = 6;
#[cfg(all(unix, not(target_vendor = "apple")))]
const CLOCK_MONOTONIC: i32 = 1;

#[cfg(unix)]
extern "C" {
    fn clock_gettime(clk: i32, tp: *mut Timespec) -> i32;
}

/// Nanoseconds on a monotonic clock. The origin is arbitrary — only
/// differences mean anything.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn rill_now_nanos() -> i64 {
    let mut t = Timespec { secs: 0, nanos: 0 };
    if clock_gettime(CLOCK_MONOTONIC, &mut t) != 0 {
        return 0;
    }
    t.secs.wrapping_mul(1_000_000_000).wrapping_add(t.nanos)
}

#[cfg(windows)]
extern "system" {
    fn QueryPerformanceCounter(count: *mut i64) -> i32;
    fn QueryPerformanceFrequency(freq: *mut i64) -> i32;
}

#[cfg(windows)]
#[no_mangle]
pub unsafe extern "C" fn rill_now_nanos() -> i64 {
    let mut c: i64 = 0;
    let mut f: i64 = 0;
    if QueryPerformanceCounter(&mut c) == 0 || QueryPerformanceFrequency(&mut f) == 0 || f == 0 {
        return 0;
    }
    // Seconds and remainder separately: `c * 1_000_000_000` overflows an i64
    // after about three hours at a 10 MHz counter, and the counter does not
    // start at zero.
    (c / f).wrapping_mul(1_000_000_000).wrapping_add((c % f).wrapping_mul(1_000_000_000) / f)
}

/// wasip1 has `clock_time_get`, but nothing that reaches this build uses it:
/// the wasm target is the browser one, where the host keeps the clock.
#[cfg(any(target_family = "wasm", target_os = "none"))]
#[no_mangle]
pub unsafe extern "C" fn rill_now_nanos() -> i64 {
    0
}

/// Milliseconds since 1970 — the clock the world agrees on, as against
/// `now_nanos`, which only agrees with itself. A server needs this to say what
/// time it is, to date a cookie, and to write down anything another machine
/// will read. `CLOCK_REALTIME` is 0 everywhere, unlike the monotonic one.
#[cfg(unix)]
#[no_mangle]
pub unsafe extern "C" fn rill_now_millis() -> i64 {
    let mut t = Timespec { secs: 0, nanos: 0 };
    if clock_gettime(0, &mut t) != 0 {
        return 0;
    }
    t.secs.wrapping_mul(1000).wrapping_add(t.nanos / 1_000_000)
}

#[cfg(windows)]
extern "system" {
    fn GetSystemTimeAsFileTime(out: *mut u64);
}

/// Windows counts hundreds of nanoseconds from 1601; 11644473600 seconds
/// separate that year from 1970.
#[cfg(windows)]
#[no_mangle]
pub unsafe extern "C" fn rill_now_millis() -> i64 {
    let mut ticks: u64 = 0;
    GetSystemTimeAsFileTime(&mut ticks);
    (ticks / 10_000) as i64 - 11_644_473_600_000
}

/// A host that keeps its own clock keeps this one too: on wasm the page has
/// `Date.now`, and a chip with no operating system has nothing to say.
#[cfg(any(target_family = "wasm", target_os = "none"))]
#[no_mangle]
pub unsafe extern "C" fn rill_now_millis() -> i64 {
    0
}

// ---- how much memory this process is holding ------------------------------
//
// Resident set size: the pages the process really has in memory at this
// moment, which is the number an operator watches while a server runs and the
// one that tells them whether it is about to be killed. Every system keeps it
// somewhere different, and none of them anywhere a program can simply read.

/// macOS keeps it in the Mach task's own accounting. `mach_task_self_` is a
/// port held in a global rather than something to call, and flavor 20 —
/// `MACH_TASK_BASIC_INFO` — lays out virtual size then resident size, both
/// 64-bit, at the head of a twelve-word struct.
#[cfg(all(unix, target_vendor = "apple"))]
#[no_mangle]
pub unsafe extern "C" fn rill_rss_bytes() -> i64 {
    extern "C" {
        static mach_task_self_: u32;
        fn task_info(task: u32, flavor: u32, out: *mut u32, count: *mut u32) -> i32;
    }
    let mut info = [0u32; 12];
    let mut words: u32 = 12;
    if task_info(mach_task_self_, 20, info.as_mut_ptr(), &mut words) != 0 {
        return 0;
    }
    ((info[2] as u64) | ((info[3] as u64) << 32)) as i64
}

/// Linux publishes it in `/proc/self/statm`, whose second field is the
/// resident page count. Pages are not always four kilobytes — arm64 servers
/// run sixteen and sixty-four — so the size is asked for rather than assumed.
#[cfg(all(unix, not(target_vendor = "apple")))]
#[no_mangle]
pub unsafe extern "C" fn rill_rss_bytes() -> i64 {
    extern "C" {
        fn sysconf(name: i32) -> i64;
    }
    let fd = open(b"/proc/self/statm\0".as_ptr(), 0);
    if fd < 0 {
        return 0; // A unix without procfs has nowhere to look.
    }
    let mut buf = [0u8; 64];
    let got = read(fd, buf.as_mut_ptr(), buf.len() - 1);
    close(fd);
    if got <= 0 {
        return 0;
    }
    let end = got as usize;
    let mut at = 0usize;
    while at < end && buf[at] != b' ' {
        at += 1; // the total size, which is not the question
    }
    while at < end && buf[at] == b' ' {
        at += 1;
    }
    let mut pages: i64 = 0;
    while at < end && buf[at] >= b'0' && buf[at] <= b'9' {
        pages = pages * 10 + (buf[at] - b'0') as i64;
        at += 1;
    }
    let page = sysconf(30); // _SC_PAGESIZE
    pages * if page > 0 { page } else { 4096 }
}

#[cfg(windows)]
#[repr(C)]
struct MemoryCounters {
    size: u32,
    page_faults: u32,
    peak_working_set: usize,
    working_set: usize,
    peak_paged_pool: usize,
    paged_pool: usize,
    peak_nonpaged_pool: usize,
    nonpaged_pool: usize,
    pagefile: usize,
    peak_pagefile: usize,
}

/// Windows calls it the working set. `K32GetProcessMemoryInfo` is the copy of
/// the call that lives in kernel32, so nothing extra has to be linked.
#[cfg(windows)]
#[no_mangle]
pub unsafe extern "C" fn rill_rss_bytes() -> i64 {
    extern "system" {
        fn GetCurrentProcess() -> *mut u8;
        fn K32GetProcessMemoryInfo(task: *mut u8, out: *mut MemoryCounters, size: u32) -> i32;
    }
    let mut counters: MemoryCounters = core::mem::zeroed();
    counters.size = core::mem::size_of::<MemoryCounters>() as u32;
    if K32GetProcessMemoryInfo(
        GetCurrentProcess(),
        &mut counters,
        core::mem::size_of::<MemoryCounters>() as u32,
    ) == 0
    {
        return 0;
    }
    counters.working_set as i64
}

/// A page's memory is the browser's business, and a chip with no operating
/// system has one program and all of it.
#[cfg(any(target_family = "wasm", target_os = "none"))]
#[no_mangle]
pub unsafe extern "C" fn rill_rss_bytes() -> i64 {
    0
}

// ---- randomness -----------------------------------------------------------
//
// Eight bytes nobody can guess, which is a different thing from eight bytes
// nobody can predict *cheaply*. `now_nanos` gives the second and not the
// first: a token minted at a knowable moment is a token that can be searched
// for, and the clock is knowable to anybody who watched when the request was
// answered. Session tokens, salts and anything else standing between a person
// and their account need the operating system's pool, which is seeded from
// physical noise and is the only source on the machine that qualifies.
//
// One `i64` at a time rather than a buffer: it needs no allocation and no
// ownership question, and a caller wanting a longer secret calls it again.
// Sixty-four bits of real entropy per call composes into whatever is wanted.

/// Eight bytes from the operating system's cryptographic pool.
///
/// Linux has `getrandom`, which draws on the same pool as `/dev/urandom` but
/// cannot fail for want of a file descriptor. It blocks only before the pool
/// is first seeded, which on a running server has long since happened.
#[cfg(target_os = "linux")]
#[no_mangle]
pub unsafe extern "C" fn rill_random_i64() -> i64 {
    extern "C" {
        fn getrandom(buf: *mut u8, buflen: usize, flags: u32) -> isize;
    }
    let mut out: i64 = 0;
    let p = &mut out as *mut i64 as *mut u8;
    let mut got = 0usize;
    // Short reads are legal. A caller that treated one as a full one would be
    // handing out secrets with known bytes in them, so this insists.
    while got < 8 {
        let n = getrandom(p.add(got), 8 - got, 0);
        if n <= 0 {
            // There is no safe fallback for this. Returning the clock would
            // look like it worked and quietly make every secret guessable.
            panic!("getrandom failed: no entropy for a secret that needs it");
        }
        got += n as usize;
    }
    out
}

/// The BSDs and macOS have `arc4random_buf`, which cannot fail and needs no
/// loop — it is documented to always deliver the whole buffer.
#[cfg(all(unix, not(target_os = "linux")))]
#[no_mangle]
pub unsafe extern "C" fn rill_random_i64() -> i64 {
    extern "C" {
        fn arc4random_buf(buf: *mut u8, nbytes: usize);
    }
    let mut out: i64 = 0;
    arc4random_buf(&mut out as *mut i64 as *mut u8, 8);
    out
}

/// Windows keeps its pool behind `bcrypt`, and nothing that builds for Windows
/// here opens a socket to need a token. Refusing is honest; a weak answer
/// would not be.
#[cfg(windows)]
#[no_mangle]
pub unsafe extern "C" fn rill_random_i64() -> i64 {
    panic!("random_i64 is not wired up on Windows");
}

/// Under wasm the host holds the entropy, and this build's wasm target is the
/// browser, where secrets are made with `crypto.getRandomValues` on the other
/// side of the boundary. Nothing in the valley's wasm asks for this.
#[cfg(any(target_family = "wasm", target_os = "none"))]
#[no_mangle]
pub unsafe extern "C" fn rill_random_i64() -> i64 {
    panic!("random_i64 is not available under wasm");
}

/// Windows has no poller here — it has no sockets here either — so there is
/// nothing to park on and the wait is the worker's. A program that needs a
/// tick and a connection at the same time is not a Windows program yet, and
/// the compiler is not in a position to say so, so this at least waits the
/// length it was asked to.
#[cfg(windows)]
#[no_mangle]
pub unsafe extern "C" fn rill_sleep_ms(ms: i64) {
    if ms > 0 {
        sleep_nanos(ms * 1_000_000);
    }
}

/// Refused at compile time on wasm, alongside the sockets and the strands:
/// there is no scheduler to park in, and a sleep that returned instantly
/// would be a program that silently runs at the wrong speed.
#[cfg(any(target_family = "wasm", target_os = "none"))]
#[no_mangle]
pub unsafe extern "C" fn rill_sleep_ms(_ms: i64) {}

// ---- the C boundary -------------------------------------------------------
//
// Rill strings are counted heap objects with a header, so they are not
// `char*`. These copy across; `rill_to_cstr` hands out plain `malloc`
// memory that the Rill side frees with `free_ptr`.

/// # Safety
/// `s` must be a live string object; the result must be freed with
/// `rill_free_ptr`.
#[no_mangle]
pub unsafe extern "C" fn rill_to_cstr(s: *const u8) -> *mut u8 {
    cstr(s)
}

/// # Safety
/// `p` must point to a NUL-terminated string; returns a new owned Rill string.
#[no_mangle]
pub unsafe extern "C" fn rill_from_cstr(p: *const u8) -> *mut u8 {
    if p.is_null() {
        return str_new(0);
    }
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    let out = str_new(n);
    copy_small(p, out.add(16), n);
    out
}

/// Bytes from C memory, taken by length rather than by a terminator, which is
/// what a library that fills a buffer and returns a count gives back.
///
/// # Safety
/// `p` must point to at least `n` readable bytes; returns a new owned Rill
/// string.
#[no_mangle]
pub unsafe extern "C" fn rill_str_from_ptr(p: *const u8, n: i64) -> *mut u8 {
    if p.is_null() || n <= 0 {
        return str_new(0);
    }
    let n = n as usize;
    let out = str_new(n);
    copy_small(p, out.add(16), n);
    out
}

/// # Safety
/// `p` must come from `rill_to_cstr` or C `malloc`.
#[no_mangle]
pub unsafe extern "C" fn rill_free_ptr(p: *mut u8) {
    if !p.is_null() {
        free(p);
    }
}

#[no_mangle]
pub extern "C" fn rill_null_ptr() -> *mut u8 {
    core::ptr::null_mut()
}

#[no_mangle]
pub extern "C" fn rill_is_null(p: *const u8) -> i8 {
    p.is_null() as i8
}

// ---- counted buffers ------------------------------------------------------
//
// The typed `Buf(w)` of the language: `{ i64 rc, i64 len, elements... }`, laid
// out like a string and counted like one, so it is released when the last
// reference goes and never freed by hand. The element width lives in the
// type, so the compiler passes it in rather than the buffer carrying it.

/// A decimal, for the one message this file has to build itself.
unsafe fn out_dec(v: i64) {
    let mut buf = [0u8; 24];
    let mut i = buf.len();
    let neg = v < 0;
    let mut u = v.unsigned_abs();
    if u == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while u > 0 {
        i -= 1;
        buf[i] = b'0' + (u % 10) as u8;
        u /= 10;
    }
    if neg {
        i -= 1;
        buf[i] = b'-';
    }
    out(&buf[i..]);
}

/// Zeroed room for `n` elements of `size` bytes. The count is in elements.
/// One bounds test for a whole loop: `v[i]` for every `i` in `lo..hi` is in
/// range when `lo >= 0` and `hi <= #v`; an empty range asks nothing. The
/// compiler puts this in front of a `for` whose body indexes `v` by the loop
/// variable and drops the test from every element.
///
/// # Safety
/// `p` must be a live buffer.
#[no_mangle]
pub unsafe extern "C" fn rill_buf_check(p: *const u8, lo: i64, hi: i64) {
    if lo >= hi {
        return;
    }
    let len = *(p as *const i64).sub(1);
    if lo < 0 {
        rill_buf_oob(lo, len);
    }
    if hi > len {
        rill_buf_oob(hi - 1, len);
    }
}

#[no_mangle]
pub unsafe extern "C" fn rill_buf_new(n: i64, size: i64) -> *mut u8 {
    if n < 0 {
        rill_buf_oob(n, 0);
    }
    // A buffer value is the address of its first element; the count and
    // the length sit in the sixteen bytes before it, so that an element is
    // one register-indexed load with nothing added.
    let bytes = (n as u64).saturating_mul(size as u64);
    // A big buffer comes from `calloc`, whose pages the kernel hands over
    // already zero and only when touched; a small one from the pool, cleared
    // with `memset`. Both replace a byte-at-a-time loop that took as long
    // over eighty megabytes as the arithmetic it was making room for.
    let block = if 16 + bytes > POOL_MAX as u64 {
        live_add(1);
        let b = match big_take((16 + bytes) as usize) {
            Some(b) => {
                memset(b.add(16), 0, bytes as usize);
                b
            }
            None => calloc(1, (16 + bytes) as usize),
        };
        if b.is_null() {
            out(b"rill: out of memory\n");
            die();
        }
        b
    } else {
        let b = rill_alloc(16 + bytes);
        memset(b.add(16), 0, bytes as usize);
        b
    };
    *(block as *mut i64) = 1;
    *(block as *mut i64).add(1) = n;
    block.add(16)
}

/// A buffer whose elements are whatever the memory held: for a caller
/// about to write every one of them, which is what an array library's
/// operations do. Skipping the clearing is a third of the cost of an
/// eighty-megabyte result; nothing else is skipped, and the count and
/// length are set as for any buffer.
///
/// # Safety
/// `n` and `size` describe the elements; the elements are not zero.
#[no_mangle]
pub unsafe extern "C" fn rill_buf_new_uninit(n: i64, size: i64) -> *mut u8 {
    if n < 0 {
        rill_buf_oob(n, 0);
    }
    let bytes = (n as u64).saturating_mul(size as u64);
    let block = if 16 + bytes > POOL_MAX as u64 {
        live_add(1);
        let b = match big_take((16 + bytes) as usize) {
            Some(b) => b,
            None => malloc((16 + bytes) as usize),
        };
        if b.is_null() {
            out(b"rill: out of memory\n");
            die();
        }
        b
    } else {
        rill_alloc(16 + bytes)
    };
    *(block as *mut i64) = 1;
    *(block as *mut i64).add(1) = n;
    block.add(16)
}

#[no_mangle]
pub unsafe extern "C" fn rill_buf_new_uninit_f64(n: i64) -> *mut u8 {
    rill_buf_new_uninit(n, 8)
}

// ---- big blocks -------------------------------------------------------------
//
// A block of a megabyte or more that a program lets go of is the block the
// next operation of the same shape is about to ask for: an array library
// makes an eighty-megabyte result, consumes it, frees it, and makes the next.
// Handing such a block back to the system and asking for it again costs a
// page fault per page — several milliseconds per result — so the last few
// are kept and handed out again to a request of the same size, or nearly.
// The system's allocator does this for C programs; this does it for Rill's.

const BIG_MIN: usize = 1 << 20;
const BIG_SLOTS: usize = 8;
static mut BIG_PTR: [*mut u8; BIG_SLOTS] = [core::ptr::null_mut(); BIG_SLOTS];
static mut BIG_SIZE: [usize; BIG_SLOTS] = [0; BIG_SLOTS];
static BIG_LOCK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

unsafe fn big_lock() {
    while BIG_LOCK
        .compare_exchange_weak(
            false,
            true,
            core::sync::atomic::Ordering::Acquire,
            core::sync::atomic::Ordering::Relaxed,
        )
        .is_err()
    {}
}

unsafe fn big_unlock() {
    BIG_LOCK.store(false, core::sync::atomic::Ordering::Release);
}

/// A kept block that fits `bytes` without wasting more than a quarter.
unsafe fn big_take(bytes: usize) -> Option<*mut u8> {
    if bytes < BIG_MIN {
        return None;
    }
    big_lock();
    let mut found = None;
    for i in 0..BIG_SLOTS {
        let have = BIG_SIZE[i];
        if !BIG_PTR[i].is_null() && have >= bytes && have <= bytes + bytes / 4 {
            found = Some(BIG_PTR[i]);
            BIG_PTR[i] = core::ptr::null_mut();
            BIG_SIZE[i] = 0;
            break;
        }
    }
    big_unlock();
    found
}

/// Keeps a freed block for the next request, or lets it go if there is no
/// room: the oldest kept block makes way, so the set follows the program.
unsafe fn big_keep(ptr: *mut u8, bytes: usize) -> bool {
    if bytes < BIG_MIN {
        return false;
    }
    big_lock();
    let mut slot = BIG_SLOTS;
    for i in 0..BIG_SLOTS {
        if BIG_PTR[i].is_null() {
            slot = i;
            break;
        }
    }
    let evicted = if slot == BIG_SLOTS {
        // All full: drop the first and shift, so the newest is last.
        let old = BIG_PTR[0];
        for i in 1..BIG_SLOTS {
            BIG_PTR[i - 1] = BIG_PTR[i];
            BIG_SIZE[i - 1] = BIG_SIZE[i];
        }
        slot = BIG_SLOTS - 1;
        old
    } else {
        core::ptr::null_mut()
    };
    BIG_PTR[slot] = ptr;
    BIG_SIZE[slot] = bytes;
    big_unlock();
    if !evicted.is_null() {
        free(evicted);
    }
    true
}

/// # Safety
/// `p` must be a live buffer whose elements are `size` bytes.
#[no_mangle]
pub unsafe extern "C" fn rill_buf_release(p: *mut u8, size: i64) {
    let block = p.sub(16);
    let n = *(block as *const i64).add(1);
    if rc_dec(block as *mut i64) == 0 {
        rill_free_sized(block, 16 + (n as u64) * (size as u64));
    }
}

/// Nothing sensible follows an index off the end, so this ends the program
/// the way an array bound violation should — saying where, since the index
/// alone rarely says why.
#[no_mangle]
pub extern "C" fn rill_buf_oob(i: i64, len: i64) -> ! {
    unsafe {
        rill_flush_out();
        err(b"rill: buffer index ");
        err_dec(i);
        err(b" out of range, length ");
        err_dec(len);
        err(b"\n");
        crash_where();
        die()
    }
}

/// A division by zero has no answer, and what the machine or the optimizer
/// made of one was a number with no sign it was wrong. It ends the program
/// the way an index off the end does, saying where.
#[no_mangle]
pub extern "C" fn rill_div_zero() -> ! {
    unsafe {
        rill_flush_out();
        err(b"rill: division by zero\n");
        crash_where();
        die()
    }
}

/// A copy between two buffers, or within one, that stays inside both:
/// `n` elements of `size` bytes from `src[from..]` to `dst[to..]`. What
/// `memmove` on two raw pointers does without asking, asked: a length that
/// came off a corrupt page ends here with a message rather than in the
/// memory past the buffer.
///
/// # Safety
/// Both must be live buffers of elements `size` bytes wide.
#[no_mangle]
pub unsafe extern "C" fn rill_buf_copy(dst: *mut u8, to: i64, src: *const u8, from: i64, n: i64, size: i64) {
    let dl = *(dst.sub(16) as *const i64).add(1);
    let sl = *(src.sub(16) as *const i64).add(1);
    if n < 0 || to < 0 || from < 0 || to > dl - n || from > sl - n {
        rill_flush_out();
        err(b"rill: buffer copy of ");
        err_dec(n);
        err(b" at ");
        err_dec(to);
        err(b" from ");
        err_dec(from);
        err(b" out of range, lengths ");
        err_dec(dl);
        err(b" and ");
        err_dec(sl);
        err(b"\n");
        crash_where();
        die()
    }
    core::ptr::copy(src.add((from * size) as usize), dst.add((to * size) as usize), (n * size) as usize);
}

/// The same for a comparison: `n` elements of `a[at..]` against `b[bt..]`
/// as bytes, -1, 0 or 1.
///
/// # Safety
/// Both must be live buffers of elements `size` bytes wide.
#[no_mangle]
pub unsafe extern "C" fn rill_buf_cmp(a: *const u8, at: i64, b: *const u8, bt: i64, n: i64, size: i64) -> i64 {
    let al = *(a.sub(16) as *const i64).add(1);
    let bl = *(b.sub(16) as *const i64).add(1);
    if n < 0 || at < 0 || bt < 0 || at > al - n || bt > bl - n {
        rill_flush_out();
        err(b"rill: buffer compare of ");
        err_dec(n);
        err(b" at ");
        err_dec(at);
        err(b" and ");
        err_dec(bt);
        err(b" out of range, lengths ");
        err_dec(al);
        err(b" and ");
        err_dec(bl);
        err(b"\n");
        crash_where();
        die()
    }
    let r = memcmp(a.add((at * size) as usize), b.add((bt * size) as usize), (n * size) as usize);
    if r < 0 { -1 } else if r > 0 { 1 } else { 0 }
}

// ---- crashes ---------------------------------------------------------------
//
// A fault the hardware reports — an address that is nobody's, a stack that
// ran off its guard page, an instruction that is not one — used to end the
// program with the operating system's one word about it. It still ends the
// program: nothing in a Rill program can be resumed from the middle of a
// write it did not finish, and a strand that faulted may hold a channel
// the others are waiting on. But it says what happened first: which
// signal, at what address, whether that address is a strand's guard page
// (a stack overflow, which is what a fault in a Rill program most often
// is), which strand, and the Rill functions on the stack — from a table the
// compiler leaves in every binary, of every function's start and name, so
// that a stripped release build reports the same names a debug one does.
//
// The handler runs on a stack of its own on every thread, since the fault
// it most wants to report is the one that left no stack to run on. With
// `RILL_CRASH_WAIT` set it then waits, saying its pid, for a debugger to
// attach; otherwise the signal is put back to what it was and the fault
// runs again, so the process ends the way the system ends a faulting
// process — a core, if the system was asked for one, and the usual exit
// status.

/// One entry of the table the compiler emits: where a function begins, and
/// its name as a C string. `rill_nsyms` says how many.
#[repr(C)]
struct RillSym {
    addr: *const u8,
    name: *const u8,
}

extern "C" {
    static rill_syms: RillSym;
    static rill_nsyms: i64;
}

/// Unbuffered, to standard error: a crash report must not sit in a buffer
/// the crash then loses.
fn err(bytes: &[u8]) {
    unsafe { write_all(2, bytes) }
}

unsafe fn err_dec(v: i64) {
    let mut buf = [0u8; 24];
    let mut i = buf.len();
    let neg = v < 0;
    let mut u = v.unsigned_abs();
    if u == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while u > 0 {
        i -= 1;
        buf[i] = b'0' + (u % 10) as u8;
        u /= 10;
    }
    if neg {
        i -= 1;
        buf[i] = b'-';
    }
    write_all(2, &buf[i..]);
}

unsafe fn err_hex(v: usize) {
    let mut buf = [0u8; 18];
    let mut i = buf.len();
    let mut u = v;
    if u == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while u > 0 {
        i -= 1;
        buf[i] = b"0123456789abcdef"[u & 15];
        u >>= 4;
    }
    i -= 1;
    buf[i] = b'x';
    i -= 1;
    buf[i] = b'0';
    write_all(2, &buf[i..]);
}

unsafe fn err_cstr(p: *const u8) {
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    write_all(2, core::slice::from_raw_parts(p, n));
}

/// The Rill function an address is inside, and how far in: the entry with
/// the greatest start at or before it, provided the address is not past
/// the next function's start. The table is not sorted — a walk of it per
/// frame is nothing, at a time when nothing else matters.
unsafe fn sym_for(addr: usize) -> Option<(*const u8, usize)> {
    let n = rill_nsyms as usize;
    let base = &raw const rill_syms;
    let mut best: usize = 0;
    let mut name: *const u8 = core::ptr::null();
    let mut next: usize = usize::MAX;
    for i in 0..n {
        let e = &*base.add(i);
        let a = e.addr as usize;
        if a <= addr {
            if a >= best {
                best = a;
                name = e.name;
            }
        } else if a < next {
            next = a;
        }
    }
    // past the nameless entry that marks the end of the program's code is
    // the runtime, which is nobody's business here
    if name.is_null() || *name == 0 || addr >= next {
        None
    } else {
        Some((name, addr - best))
    }
}

#[cfg(all(unix, not(target_os = "wasi")))]
extern "C" {
    fn getpid() -> i32;
}

/// The frame pointer as this function sees it: where the chain of frames
/// starts when the crash is a message rather than a signal.
#[cfg(target_arch = "aarch64")]
unsafe fn frame_here() -> usize {
    let fp: usize;
    core::arch::asm!("mov {}, x29", out(reg) fp, options(nomem, nostack, preserves_flags));
    fp
}

#[cfg(target_arch = "x86_64")]
unsafe fn frame_here() -> usize {
    let fp: usize;
    core::arch::asm!("mov {}, rbp", out(reg) fp, options(nomem, nostack, preserves_flags));
    fp
}

#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
unsafe fn frame_here() -> usize {
    0
}

/// The Rill functions on the stack, innermost first: `pc`, if it is one,
/// and then the return address of every frame up the chain of frame
/// pointers from `fp` — each frame's saved frame pointer and return
/// address are the two words at it, on both machines Rill runs on, and
/// every Rill function keeps the chain. Frames the runtime or the C
/// library own are left out: what a Rill program wants to know is which
/// of its own functions it was in. The walk stops at a frame pointer that
/// is not above the last, or too far above it, before it can read
/// anything that is not a frame.
unsafe fn crash_where_from(pc: usize, mut fp: usize) {
    // a run of the same frame — a recursion that ran away — is said once,
    // with its length
    let mut shown = 0;
    let mut last: (*const u8, usize) = (core::ptr::null(), 0);
    let mut repeats = 0i64;
    if pc != 0 {
        if let Some((name, off)) = sym_for(pc) {
            crash_frame(name, off, &mut last, &mut repeats, &mut shown);
        }
    }
    let mut steps = 0;
    while fp != 0 && fp & 7 == 0 && steps < 100_000 && shown < 40 {
        let next = *(fp as *const usize);
        let ret = *((fp + 8) as *const usize);
        if ret == 0 {
            break;
        }
        // a return address is one past the call; the byte before it is
        // inside the function that called
        if let Some((name, off)) = sym_for(ret - 1) {
            crash_frame(name, off, &mut last, &mut repeats, &mut shown);
        }
        if next <= fp || next - fp > 64 << 20 {
            break;
        }
        fp = next;
        steps += 1;
    }
    crash_repeats(&mut repeats);
    if shown >= 40 {
        err(b"  ...\n");
    }
    if shown == 0 {
        err(b"  (no Rill frames found)\n");
    }
}

unsafe fn crash_frame(name: *const u8, off: usize, last: &mut (*const u8, usize), repeats: &mut i64, shown: &mut i32) {
    if name == last.0 && off == last.1 {
        *repeats += 1;
        return;
    }
    crash_repeats(repeats);
    *last = (name, off);
    err(b"  in ");
    err_cstr(name);
    err(b" +");
    err_dec(off as i64);
    err(b"\n");
    *shown += 1;
}

unsafe fn crash_repeats(repeats: &mut i64) {
    if *repeats > 0 {
        err(b"  ... the same, ");
        err_dec(*repeats);
        err(b" more times\n");
        *repeats = 0;
    }
}

/// From here: for the runtime's own checks, which are calls and have the
/// frame chain at hand.
unsafe fn crash_where() {
    crash_where_from(0, frame_here());
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct SigAction {
    handler: usize,
    mask: u32,
    flags: i32,
}

#[cfg(target_os = "macos")]
#[repr(C)]
struct StackT {
    sp: *mut u8,
    size: usize,
    flags: i32,
}

#[cfg(target_os = "macos")]
const SA_CRASH: i32 = 0x0001 /* ONSTACK */ | 0x0040 /* SIGINFO */;
#[cfg(target_os = "macos")]
const SIGBUS: i32 = 10;
#[cfg(target_os = "macos")]
const SI_ADDR_OFFSET: usize = 24;

#[cfg(target_os = "linux")]
#[repr(C)]
struct SigAction {
    handler: usize,
    mask: [u64; 16],
    flags: i32,
    restorer: usize,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct StackT {
    sp: *mut u8,
    flags: i32,
    size: usize,
}

#[cfg(target_os = "linux")]
const SA_CRASH: i32 = 0x08000000u32 as i32 /* ONSTACK */ | 4 /* SIGINFO */;
#[cfg(target_os = "linux")]
const SIGBUS: i32 = 7;
#[cfg(target_os = "linux")]
const SI_ADDR_OFFSET: usize = 16;

#[cfg(any(target_os = "macos", target_os = "linux"))]
extern "C" {
    fn sigaction(sig: i32, act: *const SigAction, old: *mut SigAction) -> i32;
    fn sigaltstack(ss: *const StackT, old: *mut StackT) -> i32;
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
const CRASH_STACK: usize = 128 * 1024;

/// A stack for the handler on this thread. Every thread that runs Rill
/// code gets one: the main thread, each worker, each host thread that
/// calls in.
#[cfg(any(target_os = "macos", target_os = "linux"))]
unsafe fn crash_stack_here() {
    let p = mmap(core::ptr::null_mut(), CRASH_STACK, PROT_RW, MAP_PRIVATE_ANON, -1, 0);
    if p as isize == -1 {
        return;
    }
    let ss = StackT { sp: p, size: CRASH_STACK, flags: 0 };
    sigaltstack(&ss, core::ptr::null_mut());
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
unsafe fn crash_stack_here() {}

static CRASH_INSTALLED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// The first thread to fault reports; a second faulting while it does
/// waits, so that the report is not cut off by the process ending.
static CRASH_TAKEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The handlers, once per process, and a stack for them on this thread.
#[cfg(any(target_os = "macos", target_os = "linux"))]
unsafe fn crash_handlers_ready() {
    crash_stack_here();
    if CRASH_INSTALLED.swap(true, core::sync::atomic::Ordering::AcqRel) {
        return;
    }
    let act = SigAction {
        handler: crash_landed as usize,
        #[cfg(target_os = "macos")]
        mask: 0,
        #[cfg(target_os = "linux")]
        mask: [0; 16],
        flags: SA_CRASH,
        #[cfg(target_os = "linux")]
        restorer: 0,
    };
    for sig in [11 /* SEGV */, SIGBUS, 4 /* ILL */, 8 /* FPE */] {
        sigaction(sig, &act, core::ptr::null_mut());
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
unsafe fn crash_handlers_ready() {}

/// Whether `addr` lies in the guard page under a stack whose top is `top`
/// and whose usable size is `size` — which is what a stack overflow looks
/// like from the outside.
unsafe fn in_guard(addr: usize, top: usize, size: usize) -> bool {
    top != 0 && addr < top - size && addr >= top - size - PAGE - 16
}

/// The program counter and frame pointer at the fault, out of the
/// context the signal came with. Where they lie is one thing on each
/// system: the mcontext is pointed to on a Mac and inline on Linux, and
/// the registers are laid out as each kernel does.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
unsafe fn fault_site(ctx: *const u8) -> (usize, usize) {
    let mc = *(ctx.add(48) as *const *const u8);
    (*(mc.add(16 + 32 * 8) as *const usize), *(mc.add(16 + 29 * 8) as *const usize))
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
unsafe fn fault_site(ctx: *const u8) -> (usize, usize) {
    let mc = *(ctx.add(48) as *const *const u8);
    (*(mc.add(16 + 16 * 8) as *const usize), *(mc.add(16 + 6 * 8) as *const usize))
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
unsafe fn fault_site(ctx: *const u8) -> (usize, usize) {
    let mc = ctx.add(176);
    (*(mc.add(8 + 32 * 8) as *const usize), *(mc.add(8 + 29 * 8) as *const usize))
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
unsafe fn fault_site(ctx: *const u8) -> (usize, usize) {
    let gregs = ctx.add(40);
    (*(gregs.add(16 * 8) as *const usize), *(gregs.add(10 * 8) as *const usize))
}

#[cfg(all(any(target_os = "macos", target_os = "linux"), not(any(target_arch = "aarch64", target_arch = "x86_64"))))]
unsafe fn fault_site(_ctx: *const u8) -> (usize, usize) {
    (0, 0)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
extern "C" fn crash_landed(sig: i32, info: *const u8, ctx: *const u8) {
    unsafe {
        if CRASH_TAKEN.swap(true, core::sync::atomic::Ordering::AcqRel) {
            loop {
                sleep_nanos(1_000_000_000);
            }
        }
        rill_flush_out();
        let addr = *(info.add(SI_ADDR_OFFSET) as *const usize);
        err(b"rill: ");
        err(match sig {
            11 => b"segmentation fault".as_slice(),
            4 => b"illegal instruction".as_slice(),
            8 => b"arithmetic fault".as_slice(),
            _ => b"bus error".as_slice(),
        });
        err(b" at ");
        err_hex(addr);
        // whose stack, and was it the guard page. The worker is looked up
        // through the thread's slot, which a fault may have found garbage
        // in: a value that is not one of the workers is said and left alone.
        let raw = me();
        let first = &raw const WORKERS[0] as usize;
        let last = &raw const WORKERS[MAX_WORKERS - 1] as usize;
        let w = if raw as usize >= first && raw as usize <= last { raw } else { core::ptr::null_mut() };
        if w.is_null() && !raw.is_null() {
            err(b"\n  (the thread's worker slot holds ");
            err_hex(raw as usize);
            err(b", not a worker)");
        }
        let overflow = if !w.is_null() && in_guard(addr, (*w).exec_top, EXEC_STACK_SIZE) {
            true
        } else {
            in_guard(addr, MAIN_STACK_TOP, MAIN_STACK_SIZE)
        };
        if overflow {
            err(b" - a stack overflow: the strand ran off the end of its stack");
        }
        if !w.is_null() && !(*w).current.is_null() {
            let g = (*w).current;
            if (*g).is_main {
                err(b"\n  in the main strand");
            } else if let Some((name, _)) = sym_for((*g).code as usize) {
                err(b"\n  in a strand started at ");
                err_cstr(name);
            }
            if NWORKERS > 1 {
                err(b", worker ");
                err_dec((*w).index as i64);
            }
        }
        err(b"\n");
        let (pc, fp) = fault_site(ctx);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        if !getenv(b"RILL_CRASH_REGS\0".as_ptr()).is_null() {
            let mc = *(ctx.add(48) as *const *const u8);
            for i in 0..29 {
                err(b"  x");
                err_dec(i as i64);
                err(b" = ");
                err_hex(*(mc.add(16 + i * 8) as *const usize));
                err(b"\n");
            }
        }
        crash_where_from(pc, fp);
        let wait = getenv(b"RILL_CRASH_WAIT\0".as_ptr());
        if !wait.is_null() && *wait != 0 {
            err(b"rill: waiting for a debugger, pid ");
            err_dec(getpid() as i64);
            err(b"\n");
            loop {
                sleep_nanos(1_000_000_000);
            }
        }
        // the signal back to what the system does with it, and the
        // faulting instruction run again under that
        let dfl = SigAction {
            handler: 0,
            #[cfg(target_os = "macos")]
            mask: 0,
            #[cfg(target_os = "linux")]
            mask: [0; 16],
            flags: 0,
            #[cfg(target_os = "linux")]
            restorer: 0,
        };
        sigaction(sig, &dfl, core::ptr::null_mut());
    }
}

// ---- hash tables ----------------------------------------------------------
//
// `Map(k, v)` is storage the way a `Buf` is: open addressing, linear probing,
// a power-of-two number of slots, and one table for every key and value type
// there is. What a table cannot know about what it holds — how to hash a key,
// how to compare two, how to count a reference up and down, and how wide a
// value is — the compiler works out per instantiation and leaves in a vtable
// beside the program. Those calls are the only indirect ones, and they are why
// the same probing loop serves `Map(Str, Int)` and `Map(Point, List(Str))`.
//
// A table is two blocks in one allocation. The **control bytes** are one byte
// a slot saying whether it holds anything, and if it does, seven bits of the
// key's hash. The **slots** are the key and the value, side by side, so that
// finding a key and reading its value is one place in memory rather than two.
//
// The point of the split is that the control bytes are small: a table with two
// million slots has two megabytes of them, which stays in cache while the
// slots themselves cannot. A probe walks bytes that are there, and touches the
// slots once, for the one key it believes it has found. Keeping the whole
// 64-bit hash in the slot instead would answer the same question and cost
// eight bytes a slot to do it — half again the size of an `Int`-to-`Int`
// entry. What that costs is the growth, which has to hash every key again
// rather than reading it back; measured, that is about a tenth of the time to
// fill a table, and it buys most of the memory the table takes.
//
// A key is one word: what may be a key is what has structural equality, and a
// closure — the only value needing two — has none. A value is one word too
// unless it is a closure, and which it is comes from the vtable, so an
// `Int`-to-`Int` entry is seventeen bytes and not a byte more.

/// Nothing was ever here; a probe that reaches one is finished.
const CTRL_EMPTY: u8 = 0xFF;
/// Something was here and was deleted. A probe walks past it and an insert may
/// take it — without this a deletion would cut the chain a later key probed
/// along.
const CTRL_TOMB: u8 = 0xFE;
const MAP_MIN_CAP: usize = 8;

/// The seven bits of the hash a slot keeps, which is what a probe compares
/// before it goes anywhere near the key. Taken from the top, because the
/// bottom bits already chose the bucket.
#[inline]
fn ctrl_of(h: u64) -> u8 {
    (h >> 57) as u8
}

#[inline]
fn is_full(c: u8) -> bool {
    c < 0x80
}

/// The per-instantiation operations, emitted by the compiler as a constant.
#[repr(C)]
pub struct MapVt {
    hash: unsafe extern "C" fn(i64) -> i64,
    eq: unsafe extern "C" fn(i64, i64) -> i8,
    kretain: unsafe extern "C" fn(i64),
    krelease: unsafe extern "C" fn(i64),
    /// Called with as many words as `vwords` says.
    vretain: unsafe extern "C" fn(i64, i64),
    vrelease: unsafe extern "C" fn(i64, i64),
    /// 1 for every value type but a closure, which needs its code and its
    /// environment.
    vwords: i64,
    /// What the two types are, when it lets the table skip the calls above:
    /// `KIND_INT_KEY` hashes and compares the key here instead of through
    /// `hash` and `eq` (the same mix the compiler emits, so a table grown
    /// here and probed there agree), and `KIND_SCALAR_VAL` says the value
    /// has no count to touch. Two indirect calls per probe and one per read
    /// were most of what made a lookup 2.4× a hand-written C table's.
    kind: i64,
}

const KIND_INT_KEY: i64 = 1;
const KIND_SCALAR_VAL: i64 = 2;

/// The compiler's `hash_int`, bit for bit.
#[inline]
fn hash_int(k: u64) -> u64 {
    let m = k.wrapping_mul(0x9e37_79b9_7f4a_7c15);
    m ^ (m >> 29)
}

#[inline]
unsafe fn map_hash(m: *const Map, k: i64) -> u64 {
    let vt = (*m).vt;
    if (*vt).kind & KIND_INT_KEY != 0 {
        hash_int(k as u64)
    } else {
        ((*vt).hash)(k) as u64
    }
}

#[inline]
unsafe fn map_key_eq(m: *const Map, stored: i64, k: i64) -> bool {
    let vt = (*m).vt;
    if (*vt).kind & KIND_INT_KEY != 0 {
        stored == k
    } else {
        ((*vt).eq)(stored, k) != 0
    }
}

#[inline]
unsafe fn map_vretain(m: *const Map, v0: i64, v1: i64) {
    let vt = (*m).vt;
    if (*vt).kind & KIND_SCALAR_VAL == 0 {
        ((*vt).vretain)(v0, v1);
    }
}

#[inline]
unsafe fn map_vrelease(m: *const Map, v0: i64, v1: i64) {
    let vt = (*m).vt;
    if (*vt).kind & KIND_SCALAR_VAL == 0 {
        ((*vt).vrelease)(v0, v1);
    }
}

#[inline]
unsafe fn map_krelease(m: *const Map, k: i64) {
    let vt = (*m).vt;
    if (*vt).kind & KIND_INT_KEY == 0 {
        ((*vt).krelease)(k);
    }
}

#[inline]
unsafe fn map_val1(m: *const Map, slot: *const i64) -> i64 {
    if (*m).stride > 2 {
        *slot.add(2)
    } else {
        0
    }
}

#[repr(C)]
struct Map {
    rc: i64,
    vt: *const MapVt,
    ctrl: *mut u8,
    /// `cap * stride` words: the key, then the value.
    slots: *mut i64,
    cap: usize,
    /// Words a slot takes: the key and the value.
    stride: usize,
    /// Live entries.
    len: usize,
    /// Live entries plus tombstones — what the load factor is really about,
    /// since a tombstone lengthens a probe exactly as an entry does.
    used: usize,
}

/// The one allocation: control bytes, padded to a word, then the slots.
#[inline]
fn map_block_bytes(cap: usize, stride: usize) -> u64 {
    ((cap + 7) & !7) as u64 + (cap * stride * 8) as u64
}

unsafe fn map_alloc_block(m: *mut Map, cap: usize) {
    let p = rill_alloc(map_block_bytes(cap, (*m).stride));
    let mut i = 0;
    while i < cap {
        *p.add(i) = CTRL_EMPTY;
        i += 1;
    }
    (*m).ctrl = p;
    (*m).slots = p.add((cap + 7) & !7) as *mut i64;
    (*m).cap = cap;
}

#[inline]
unsafe fn map_key(m: *const Map, i: usize) -> *mut i64 {
    (*m).slots.add(i * (*m).stride)
}

/// The slot holding `k`, or -1. The control byte answers for all but one slot
/// in a hundred and twenty-eight without the key being read at all.
unsafe fn map_lookup(m: *const Map, h: u64, k: i64) -> isize {
    let cap = (*m).cap;
    if cap == 0 {
        return -1;
    }
    let mask = cap - 1;
    let want = ctrl_of(h);
    let mut i = (h as usize) & mask;
    loop {
        let c = *(*m).ctrl.add(i);
        if c == CTRL_EMPTY {
            return -1;
        }
        if c == want && map_key_eq(m, *map_key(m, i), k) {
            return i as isize;
        }
        i = (i + 1) & mask;
    }
}

/// Moves every live entry into a fresh block of `want` slots (or more), which
/// is also how tombstones are cleared. Every key is hashed again, since a slot
/// keeps seven bits of its hash and not the rest. No reference count moves: an
/// entry keeps the two it already held.
unsafe fn map_grow(m: *mut Map, want: usize) {
    let mut cap = MAP_MIN_CAP;
    while cap < want {
        cap *= 2;
    }
    let (octrl, oslots, ocap) = ((*m).ctrl, (*m).slots, (*m).cap);
    let stride = (*m).stride;
    map_alloc_block(m, cap);
    let mask = cap - 1;
    let mut i = 0;
    while i < ocap {
        if is_full(*octrl.add(i)) {
            let from = oslots.add(i * stride);
            let h = map_hash(m, *from);
            let mut j = (h as usize) & mask;
            while *(*m).ctrl.add(j) != CTRL_EMPTY {
                j = (j + 1) & mask;
            }
            *(*m).ctrl.add(j) = ctrl_of(h);
            let to = map_key(m, j);
            let mut w = 0;
            while w < stride {
                *to.add(w) = *from.add(w);
                w += 1;
            }
        }
        i += 1;
    }
    (*m).used = (*m).len;
    if ocap != 0 {
        rill_free_sized(octrl, map_block_bytes(ocap, stride));
    }
}

/// # Safety
/// `vt` must outlive every table made with it (the compiler emits it as a
/// constant global). Returns an owned, empty map (rc = 1) that has not
/// allocated any slots yet.
#[no_mangle]
pub unsafe extern "C" fn rill_map_new(vt: *const MapVt) -> *mut u8 {
    let m = rill_alloc(core::mem::size_of::<Map>() as u64) as *mut Map;
    core::ptr::write(
        m,
        Map {
            rc: 1,
            vt,
            ctrl: core::ptr::null_mut(),
            slots: core::ptr::null_mut(),
            cap: 0,
            stride: 1 + (*vt).vwords as usize,
            len: 0,
            used: 0,
        },
    );
    m as *mut u8
}

/// Inserts or replaces. The key and the value arrive owned: on a replacement
/// the old value is released and so is the incoming key, since the key that
/// is already there stays.
///
/// # Safety
/// `p` must be a live map holding this key and value type.
#[no_mangle]
pub unsafe extern "C" fn rill_map_set(p: *mut u8, k: i64, v0: i64, v1: i64) {
    let m = p as *mut Map;
    let h = map_hash(m, k);
    // One probe finds the key, or the place it would go: the first
    // tombstone passed, else the empty slot the probe ended on.
    if (*m).cap != 0 {
        let mask = (*m).cap - 1;
        let want = ctrl_of(h);
        let mut i = (h as usize) & mask;
        let mut tomb: isize = -1;
        loop {
            let c = *(*m).ctrl.add(i);
            if c == CTRL_EMPTY {
                break;
            }
            if c == CTRL_TOMB {
                if tomb < 0 {
                    tomb = i as isize;
                }
            } else if c == want && map_key_eq(m, *map_key(m, i), k) {
                let slot = map_key(m, i);
                map_vrelease(m, *slot.add(1), map_val1(m, slot));
                *slot.add(1) = v0;
                if (*m).stride > 2 {
                    *slot.add(2) = v1;
                }
                map_krelease(m, k);
                return;
            }
            i = (i + 1) & mask;
        }
        if ((*m).used + 1) * 4 < (*m).cap * 3 {
            let at = if tomb >= 0 {
                tomb as usize
            } else {
                (*m).used += 1;
                i
            };
            *(*m).ctrl.add(at) = ctrl_of(h);
            let slot = map_key(m, at);
            *slot = k;
            *slot.add(1) = v0;
            if (*m).stride > 2 {
                *slot.add(2) = v1;
            }
            (*m).len += 1;
            return;
        }
    }
    // Too full, or no room yet: rebuild — which also clears the tombstones
    // — and place the key in the fresh table, where every slot it passes is
    // live or empty.
    let want = if ((*m).len + 1) * 4 >= (*m).cap * 3 { (*m).cap * 2 } else { (*m).cap };
    map_grow(m, want);
    let mask = (*m).cap - 1;
    let mut i = (h as usize) & mask;
    while *(*m).ctrl.add(i) != CTRL_EMPTY {
        i = (i + 1) & mask;
    }
    (*m).used += 1;
    *(*m).ctrl.add(i) = ctrl_of(h);
    let slot = map_key(m, i);
    *slot = k;
    *slot.add(1) = v0;
    if (*m).stride > 2 {
        *slot.add(2) = v1;
    }
    (*m).len += 1;
}

/// The value under `k`, or `d`: `map_find` and `map_val_at` in one call, for
/// the one-word values the prelude's `map_or` is asked about most.
///
/// # Safety
/// `p` must be a live map whose values are one word.
#[no_mangle]
pub unsafe extern "C" fn rill_map_get_or(p: *mut u8, k: i64, d: i64) -> i64 {
    let m = p as *mut Map;
    let h = map_hash(m, k);
    let at = map_lookup(m, h, k);
    if at < 0 {
        return d;
    }
    let v = *map_key(m, at as usize).add(1);
    map_vretain(m, v, 0);
    v
}

/// The slot holding `k`, or -1. The key is borrowed.
///
/// # Safety
/// `p` must be a live map holding this key type.
#[no_mangle]
pub unsafe extern "C" fn rill_map_find(p: *mut u8, k: i64) -> i64 {
    let m = p as *mut Map;
    let h = map_hash(m, k);
    map_lookup(m, h, k) as i64
}

/// Removes `k` if it is there, saying whether it was. The key is borrowed;
/// the key and value the table held are released.
///
/// # Safety
/// `p` must be a live map holding this key type.
#[no_mangle]
pub unsafe extern "C" fn rill_map_del(p: *mut u8, k: i64) -> i8 {
    let m = p as *mut Map;
    let h = map_hash(m, k);
    let at = map_lookup(m, h, k);
    if at < 0 {
        return 0;
    }
    let slot = map_key(m, at as usize);
    map_krelease(m, *slot);
    map_vrelease(m, *slot.add(1), map_val1(m, slot));
    *(*m).ctrl.add(at as usize) = CTRL_TOMB;
    (*m).len -= 1;
    1
}

/// # Safety
/// `p` must be a live map.
#[no_mangle]
pub unsafe extern "C" fn rill_map_len(p: *mut u8) -> i64 {
    (*(p as *const Map)).len as i64
}

/// The first slot at or after `from` that holds an entry, or -1. This is what
/// walking a map is written with.
///
/// # Safety
/// `p` must be a live map.
#[no_mangle]
pub unsafe extern "C" fn rill_map_next(p: *mut u8, from: i64) -> i64 {
    let m = p as *const Map;
    let mut i = if from < 0 { 0 } else { from as usize };
    while i < (*m).cap {
        if is_full(*(*m).ctrl.add(i)) {
            return i as i64;
        }
        i += 1;
    }
    -1
}

unsafe fn map_slot(m: *const Map, i: i64) -> *mut i64 {
    if i < 0 || i as usize >= (*m).cap || !is_full(*(*m).ctrl.add(i as usize)) {
        out(b"rill: map slot ");
        out_dec(i);
        out(b" holds nothing\n");
        die()
    }
    map_key(m, i as usize)
}

/// The key in a slot, owned by the caller.
///
/// # Safety
/// `p` must be a live map and `i` a slot holding an entry.
#[no_mangle]
pub unsafe extern "C" fn rill_map_key_at(p: *mut u8, i: i64) -> i64 {
    let m = p as *const Map;
    let slot = map_slot(m, i);
    ((*(*m).vt).kretain)(*slot);
    *slot
}

/// The value in a slot, owned by the caller. The second word is nothing when
/// the value type does not use one.
///
/// # Safety
/// `p` must be a live map and `i` a slot holding an entry.
#[no_mangle]
pub unsafe extern "C" fn rill_map_val_at(p: *mut u8, i: i64) -> TwoWords {
    let m = p as *const Map;
    let slot = map_slot(m, i);
    let (v0, v1) = (*slot.add(1), if (*m).stride > 2 { *slot.add(2) } else { 0 });
    map_vretain(m, v0, v1);
    TwoWords { a: v0 as u64, b: v1 as u64 }
}

/// Releases every entry and leaves an empty table of the same capacity, which
/// is what a map reused round a loop wants instead of a fresh one.
unsafe fn map_drop_entries(m: *mut Map) {
    let _ = (*m).vt;
    let mut i = 0;
    while i < (*m).cap {
        if is_full(*(*m).ctrl.add(i)) {
            let slot = map_key(m, i);
            map_krelease(m, *slot);
            map_vrelease(m, *slot.add(1), map_val1(m, slot));
        }
        *(*m).ctrl.add(i) = CTRL_EMPTY;
        i += 1;
    }
    (*m).len = 0;
    (*m).used = 0;
}

/// # Safety
/// `p` must be a live map.
#[no_mangle]
pub unsafe extern "C" fn rill_map_clear(p: *mut u8) {
    map_drop_entries(p as *mut Map);
}

/// # Safety
/// `p` must be a live map.
#[no_mangle]
pub unsafe extern "C" fn rill_map_release(p: *mut u8) {
    let m = p as *mut Map;
    if rc_dec(p as *mut i64) != 0 {
        return;
    }
    let (cap, stride) = ((*m).cap, (*m).stride);
    map_drop_entries(m);
    if cap != 0 {
        rill_free_sized((*m).ctrl, map_block_bytes(cap, stride));
    }
    rill_free_sized(p, core::mem::size_of::<Map>() as u64);
}

/// FNV-1a over a string's bytes. The compiler hashes everything else inline —
/// a string is the one key whose hashing is a loop worth keeping in one place.
///
/// # Safety
/// `s` must be a live string object.
#[no_mangle]
pub unsafe extern "C" fn rill_str_hash(s: *const u8) -> i64 {
    let n = str_len_of(s);
    let b = str_bytes(s);
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < n {
        h ^= *b.add(i) as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
    }
    h as i64
}

// ---- raw buffers ----------------------------------------------------------
//
// A C library that wants an array — vertex data, a pixel buffer, a matrix —
// needs memory Rill can fill element by element. These work on plain `malloc`
// memory outside the reference counter, so a buffer is freed with `free_ptr`
// exactly like the result of `to_cstr`.
//
// The index is in *elements*, not bytes: `poke_f32(p, 3, x)` writes the fourth
// float. `ptr_offset` is the escape hatch when a layout needs byte addressing,
// and every access is unaligned-safe so an offset pointer stays valid.

/// Zeroed C memory, `n` bytes of it, or null if `n` is not positive.
/// The caller frees it with `free_ptr`.
#[no_mangle]
pub extern "C" fn rill_alloc_bytes(n: i64) -> *mut u8 {
    if n <= 0 {
        return core::ptr::null_mut();
    }
    unsafe { calloc(n as usize, 1) }
}

/// The result stays valid only as long as `p` does, and must not itself be
/// freed. Offsetting null is deliberately allowed and does not read anything:
/// OpenGL and friends pass a byte offset in a pointer-shaped argument, and
/// `ptr_offset(null_ptr(), 8)` is how that is written.
#[no_mangle]
pub extern "C" fn rill_ptr_offset(p: *mut u8, bytes: i64) -> *mut u8 {
    p.wrapping_offset(bytes as isize)
}

/// Writes and reads. Each takes an element index; the runtime does the
/// narrowing, so the Rill side always deals in `Int` and `Float`.
macro_rules! poke {
    ($name:ident, $t:ty, $v:ty) => {
        /// # Safety
        /// `p` must point to a buffer holding at least `i + 1` elements.
        #[no_mangle]
        pub unsafe extern "C" fn $name(p: *mut u8, i: i64, v: $v) {
            let slot = p.offset(i as isize * core::mem::size_of::<$t>() as isize);
            (slot as *mut $t).write_unaligned(v as $t);
        }
    };
}

macro_rules! peek {
    ($name:ident, $t:ty, $v:ty) => {
        /// # Safety
        /// `p` must point to a buffer holding at least `i + 1` elements.
        #[no_mangle]
        pub unsafe extern "C" fn $name(p: *const u8, i: i64) -> $v {
            let slot = p.offset(i as isize * core::mem::size_of::<$t>() as isize);
            (slot as *const $t).read_unaligned() as $v
        }
    };
}

/// # Safety
/// `p` must point to a buffer holding at least `i + 1` pointers. Storing a
/// pointer is how an array of C strings — `char**` — gets built.
#[no_mangle]
pub unsafe extern "C" fn rill_poke_ptr(p: *mut u8, i: i64, v: *mut u8) {
    let slot = p.offset(i as isize * core::mem::size_of::<*mut u8>() as isize);
    (slot as *mut *mut u8).write_unaligned(v);
}

/// # Safety
/// `p` must point to a buffer holding at least `i + 1` pointers. This is how
/// a C out-parameter that returns a handle is read back.
#[no_mangle]
pub unsafe extern "C" fn rill_peek_ptr(p: *const u8, i: i64) -> *mut u8 {
    let slot = p.offset(i as isize * core::mem::size_of::<*mut u8>() as isize);
    (slot as *const *mut u8).read_unaligned()
}

poke!(rill_poke_i8, i8, i64);
poke!(rill_poke_i16, i16, i64);
poke!(rill_poke_i32, i32, i64);
poke!(rill_poke_i64, i64, i64);
poke!(rill_poke_f32, f32, f64);
poke!(rill_poke_f64, f64, f64);

peek!(rill_peek_i8, i8, i64);
peek!(rill_peek_u8, u8, i64);
peek!(rill_peek_i16, i16, i64);
peek!(rill_peek_u16, u16, i64);
peek!(rill_peek_i32, i32, i64);
peek!(rill_peek_u32, u32, i64);
peek!(rill_peek_i64, i64, i64);
peek!(rill_peek_f32, f32, f64);
peek!(rill_peek_f64, f64, f64);

// ---- print without newline ------------------------------------------------

/// # Safety
/// `s` must be a live string object.
#[no_mangle]
pub unsafe extern "C" fn rill_print_str(s: *const u8) {
    out(core::slice::from_raw_parts(str_bytes(s), str_len_of(s)));
}

#[no_mangle]
pub extern "C" fn rill_print_int(v: i64) {
    let mut buf = [0u8; 24];
    let i = dec(v, &mut buf);
    out(&buf[i..]);
}

#[no_mangle]
pub extern "C" fn rill_print_float(v: f64) {
    let mut buf = [0u8; 40];
    let n = unsafe { snprintf(buf.as_mut_ptr(), 39, b"%g\0".as_ptr(), v) };
    let n = n.clamp(0, 39) as usize;
    out(&buf[..n]);
}

#[no_mangle]
pub extern "C" fn rill_print_bool(v: i8) {
    out(if v != 0 { b"true" as &[u8] } else { b"false" });
}

#[no_mangle]
pub extern "C" fn rill_println_int(v: i64) {
    let mut buf = [0u8; 24];
    let mut i = buf.len() - 1;
    buf[i] = b'\n';
    let neg = v < 0;
    let mut u = v.unsigned_abs();
    if u == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while u > 0 {
        i -= 1;
        buf[i] = b'0' + (u % 10) as u8;
        u /= 10;
    }
    if neg {
        i -= 1;
        buf[i] = b'-';
    }
    out(&buf[i..]);
}

#[no_mangle]
pub extern "C" fn rill_println_float(v: f64) {
    let mut buf = [0u8; 40];
    let n = unsafe { snprintf(buf.as_mut_ptr(), 39, b"%g\0".as_ptr(), v) };
    let n = n.clamp(0, 38) as usize;
    buf[n] = b'\n';
    out(&buf[..n + 1]);
}

#[no_mangle]
pub extern "C" fn rill_println_bool(v: i8) {
    out(if v != 0 { b"true\n" } else { b"false\n" });
}

// ---- memory: Perceus-style reference counting (M3) ------------------------
//
// Every heap object starts with an `i64` reference count at offset 0.
// ADT boxes:     { i64 rc, i64 tag, fields... } — released by a compiler-
//                generated, type-specific drop function.
// Closure envs:  { i64 rc, ptr drop_fn, captures... } — released dynamically
//                through the stored drop function, since closures of the same
//                type can carry different environments.
//
// Counts are non-atomic until the M4 scheduler introduces cross-thread
// sharing (which will promote shared values to atomic counts).

/// Live allocation counter used to prove leak-freedom in tests.
/// Allocations minus frees. Named for the compiler, which keeps it itself on
/// the allocation fast path it lays into generated code (see `rill_pools`).
#[no_mangle]
pub static mut rill_live_allocs: i64 = 0;

/// Strands that had started running and had not finished when `main` returned.
/// Counted so that what they left behind can be told apart from a mistake.
static mut ABANDONED: i64 = 0;

// Reference counts are plain loads and stores while the program runs on one
// worker, and atomic once it asks for more (`rill_mt`, set before any thread
// starts). Generated code makes the same choice inline.

/// A count at or above this belongs to a compile-time constant — a string
/// literal or a nullary constructor — which the compiler emits into read-only
/// memory. Writing to one would fault, and it can never be freed anyway.
const IMMORTAL_MIN: i64 = (i64::MAX / 2) / 2;

#[inline(always)]
unsafe fn rc_inc(p: *mut i64) {
    if *p >= IMMORTAL_MIN {
        return;
    }
    if rill_mt == 0 {
        *p += 1;
    } else {
        (*(p as *const RcAtomic))
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

/// One counter per worker once there are several: every worker adding to
/// a single atomic put the one cache line it lives on under eight cores at
/// once, and a program that did nothing but build cons cells on eight
/// strands ran three times slower than on one. A worker counts its own
/// allocations and whatever it frees, wherever that came from, so the sum
/// over the pools is still the number of live objects. A thread that is not
/// a worker (there are none that allocate today) falls back to the global.
#[inline(always)]
unsafe fn live_add(delta: i64) {
    if rill_mt == 0 {
        rill_live_allocs += delta;
    } else {
        let w = me();
        if w.is_null() {
            (*(&raw const rill_live_allocs as *const RcAtomic))
                .fetch_add(delta, core::sync::atomic::Ordering::Relaxed);
        } else {
            (*(&raw mut rill_pools[(*w).index])).live += delta;
        }
    }
}

/// The live-allocation count, the global plus every worker's share.
unsafe fn live_total() -> i64 {
    let mut n = rill_live_allocs;
    for i in 0..MAX_WORKERS {
        n += (*(&raw const rill_pools[i])).live;
    }
    n
}

/// Decrements and returns the new value.
#[inline(always)]
unsafe fn rc_dec(p: *mut i64) -> i64 {
    if *p >= IMMORTAL_MIN {
        return *p;
    }
    if rill_mt == 0 {
        *p -= 1;
        *p
    } else {
        (*(p as *const RcAtomic))
            .fetch_sub(1, core::sync::atomic::Ordering::AcqRel)
            - 1
    }
}

// Small-object pool: size classes in 16-byte steps up to 256 bytes, backed by
// free lists over bump-allocated mmap chunks. A freed block's first word holds
// the free-list link (it is the refcount slot of a dead object, so reusing it
// is safe). Larger objects fall through to malloc/free — which is why every
// release path passes the object size (`rill_free_sized`).
const POOL_MAX: usize = 256;
const POOL_CLASSES: usize = POOL_MAX / 16;
const POOL_CHUNK: usize = 16 * 1024 * 1024;

/// One worker's share of the pool. Per worker rather than global so that
/// several workers can allocate at once without a lock — a block freed on a
/// different worker than it came from simply joins that worker's free list.
/// `repr(C)` and exported because the compiler reaches into the first pool
/// directly: a single-threaded build pops and pushes `free[class]` in
/// generated code and only calls in here when a list is empty. The free
/// lists have to stay the first field for that.
#[repr(C)]
pub struct Pool {
    free: [*mut u8; POOL_CLASSES],
    next: usize,
    end: usize,
    live: i64,
}

const POOL_INIT: Pool = Pool {
    free: [core::ptr::null_mut(); POOL_CLASSES],
    next: 0,
    end: 0,
    live: 0,
};

#[no_mangle]
pub static mut rill_pools: [Pool; MAX_WORKERS] = [POOL_INIT; MAX_WORKERS];

/// `RILL_ALLOC_CHECK=1`: every pointer that goes on a free list, and every
/// one that comes off, is checked against the chunks the pools were
/// given — a pointer outside them is a free of something that was never
/// allocated here, or a list head that something wrote over after the
/// block was freed — and the program stops there, naming the frames,
/// rather than wherever the bad pointer is next dereferenced.
static mut ALLOC_CHECK: bool = false;
/// `RILL_ALLOC_CHECK=2`: on top of the above, every small block (up to
/// `QUAR_MAX` bytes) gets a page of its own, and the page is made
/// inaccessible the moment the block is freed. A stale holder that then
/// touches the block faults right there, on its own instruction, and the
/// crash report names it — instead of the fault landing on whoever pops
/// the block next. The last `QUAR_RING` freed pages stay fenced; older
/// ones are given back.
static mut ALLOC_QUAR: bool = false;
const QUAR_MAX: usize = 64;
const QUAR_PAGE: usize = 16384;
const QUAR_RING: usize = 8192;
static QUAR_PAGES: [core::sync::atomic::AtomicUsize; QUAR_RING] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; QUAR_RING];
static QUAR_NEXT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(unix)]
unsafe fn quar_alloc() -> *mut u8 {
    let p = mmap(core::ptr::null_mut(), QUAR_PAGE, PROT_RW, MAP_PRIVATE_ANON, -1, 0);
    if p as isize == -1 {
        out(b"rill: out of memory\n");
        die();
    }
    p
}

#[cfg(unix)]
unsafe fn quar_free(ptr: *mut u8) {
    mprotect(ptr, QUAR_PAGE, PROT_NONE);
    let i = QUAR_NEXT.fetch_add(1, core::sync::atomic::Ordering::AcqRel) % QUAR_RING;
    let old = QUAR_PAGES[i].swap(ptr as usize, core::sync::atomic::Ordering::AcqRel);
    if old != 0 {
        munmap(old as *mut u8, QUAR_PAGE);
    }
}
/// One bit per 16 bytes of the first `LIVE_CHUNKS` chunks: set while the
/// block is out, cleared at its free. A block handed out while its bit is
/// set has been handed out twice — two owners of one block — and a free of
/// a block whose bit is clear is a free of something not out.
const LIVE_CHUNKS: usize = 64;
static LIVE: [core::sync::atomic::AtomicU64; LIVE_CHUNKS * 16384] =
    [const { core::sync::atomic::AtomicU64::new(0) }; LIVE_CHUNKS * 16384];

unsafe fn live_bit(p: usize) -> Option<(usize, u64)> {
    let n = NCHUNKS.load(core::sync::atomic::Ordering::Acquire).min(LIVE_CHUNKS);
    for i in 0..n {
        let (b, l) = CHUNKS[i];
        if p >= b && p < b + l {
            let k = i * (POOL_CHUNK / 16) + (p - b) / 16;
            return Some((k / 64, 1u64 << (k % 64)));
        }
    }
    None
}

unsafe fn live_out(p: usize, class: usize) {
    if let Some((w, bit)) = live_bit(p) {
        let was = LIVE[w].fetch_or(bit, core::sync::atomic::Ordering::AcqRel);
        if was & bit != 0 {
            rill_flush_out();
            err(b"rill: a block was handed out while still out: ");
            err_hex(p);
            err(b"\n");
            free_site_of(p);
            alloc_complain(b"", 0, class);
        }
    }
}

unsafe fn live_in(p: usize, class: usize) {
    if let Some((w, bit)) = live_bit(p) {
        let was = LIVE[w].fetch_and(!bit, core::sync::atomic::Ordering::AcqRel);
        if was & bit == 0 {
            rill_flush_out();
            err(b"rill: a block was freed while not out: ");
            err_hex(p);
            err(b"\n");
            free_site_of(p);
            alloc_complain(b"", 0, class);
        }
    }
}
const MAX_CHUNKS: usize = 4096;
static mut CHUNKS: [(usize, usize); MAX_CHUNKS] = [(0, 0); MAX_CHUNKS];
static NCHUNKS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
/// Chunks mapped by every pool, checks or not: for `rill_mem_report`.
static NCHUNKS_ALL: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

unsafe fn chunk_note(base: usize, len: usize) {
    let i = NCHUNKS.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
    if i < MAX_CHUNKS {
        CHUNKS[i] = (base, len);
    }
}

unsafe fn in_chunks(p: usize) -> bool {
    let n = NCHUNKS.load(core::sync::atomic::Ordering::Acquire).min(MAX_CHUNKS);
    for i in 0..n {
        let (b, l) = CHUNKS[i];
        if p >= b && p < b + l {
            return true;
        }
    }
    false
}

/// Where each block was last freed, by the block's address: the frames
/// of the free, for a reader of a freed block to be shown its freer.
#[no_mangle]
pub static mut rill_free_sites: [[usize; 10]; 65536] = [[0; 10]; 65536];
/// The free before that one, for a block that went round once between
/// its wrongful free and the touch that showed it.
static mut FREE_SITES_BEFORE: [[usize; 10]; 65536] = [[0; 10]; 65536];

unsafe fn note_free(ptr: usize, link: usize) {
    let slot = &mut rill_free_sites[(ptr >> 4) & 0xffff];
    if slot[0] == ptr {
        // freed, and not handed out again since: freed twice
        rill_flush_out();
        err(b"rill: a block was freed twice: ");
        err_hex(ptr);
        err(b"\n");
        free_site_of(ptr);
        err(b"  freed again by:\n");
        crash_where();
        die();
    }
    if slot[0] == ptr | 1 {
        // the same block, handed out again since: keep that free as the one before
        FREE_SITES_BEFORE[(ptr >> 4) & 0xffff] = *slot;
    }
    slot[0] = ptr;
    slot[9] = link;
    let mut fp = frame_here();
    for k in 1..9 {
        if fp == 0 || fp & 7 != 0 {
            slot[k] = 0;
            continue;
        }
        let ret = *((fp + 8) as *const usize);
        let next = *(fp as *const usize);
        slot[k] = ret;
        fp = if next > fp && next - fp < 64 << 20 { next } else { 0 };
    }
}

/// `RILL_FREE_OF=<hex address>`: at a crash, the frames of the last free
/// of the block at that address, from the table above.
unsafe fn free_site_of(ptr: usize) {
    let slot = &rill_free_sites[(ptr >> 4) & 0xffff];
    if slot[0] & !1 != ptr {
        err(b"  (no free of that block on record)\n");
        return;
    }
    err(if slot[0] & 1 != 0 { b"  last freed (and handed out again since) by:\n" } else { b"  last freed by:\n" });
    free_frames(slot);
    let before = &FREE_SITES_BEFORE[(ptr >> 4) & 0xffff];
    if before[0] & !1 == ptr {
        err(b"  and freed before that by:\n");
        free_frames(before);
    }
}

unsafe fn free_frames(slot: &[usize; 10]) {
    for k in 1..9 {
        if slot[k] == 0 {
            break;
        }
        if let Some((name, off)) = sym_for(slot[k] - 1) {
            err(b"    in ");
            err_cstr(name);
            err(b" +");
            err_dec(off as i64);
            err(b"\n");
        }
    }
}

/// A ring of every count update, allocation and free seen under
/// `RILL_RC_CHECK`: the block, what was done to it (0 retain, 1 release,
/// 2 release's last-owner test, 3 in-place reuse test, 4 reuse-keep test,
/// 5 handed out, 6 freed), the worker, and four frames. At a report, the
/// entries for the block in question are the block's history.
const RC_LOG: usize = 1 << 21;
static mut RC_EVENTS: [[usize; 6]; RC_LOG] = [[0; 6]; RC_LOG];
static RC_NEXT: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

unsafe fn rc_log(p: usize, kind: usize) {
    let i = RC_NEXT.fetch_add(1, core::sync::atomic::Ordering::AcqRel) & (RC_LOG - 1);
    let e = &mut RC_EVENTS[i];
    e[0] = p;
    e[1] = kind | ((*me()).index << 8);
    let mut fp = frame_here();
    for k in 2..6 {
        if fp == 0 || fp & 7 != 0 {
            e[k] = 0;
            continue;
        }
        e[k] = *((fp + 8) as *const usize);
        let next = *(fp as *const usize);
        fp = if next > fp && next - fp < 64 << 20 { next } else { 0 };
    }
}

unsafe fn rc_history(p: usize) {
    err(b"  the block's history, oldest first:\n");
    let end = RC_NEXT.load(core::sync::atomic::Ordering::Acquire);
    let start = if end > RC_LOG { end - RC_LOG } else { 0 };
    for i in start..end {
        let e = &RC_EVENTS[i & (RC_LOG - 1)];
        if e[0] != p {
            continue;
        }
        err(match e[1] & 0xff {
            0 => b"    retain  ",
            1 => b"    release ",
            2 => b"    last?   ",
            3 => b"    alone?  ",
            4 => b"    keep?   ",
            5 => b"    alloc   ",
            _ => b"    free    ",
        });
        err(b"w");
        err_dec((e[1] >> 8) as i64);
        for k in 2..6 {
            if e[k] == 0 {
                break;
            }
            if let Some((name, off)) = sym_for(e[k] - 1) {
                err(b" < ");
                err_cstr(name);
                err(b"+");
                err_dec(off as i64);
            }
        }
        err(b"\n");
    }
}

/// From code compiled with `RILL_RC_CHECK`, before every count update:
/// a block that is not out (freed, and not handed out since) is reported
/// here, in the toucher's frames, which are the ones wanted.
#[no_mangle]
pub unsafe extern "C" fn rill_rc_stale(p: *mut u8, kind: u64) {
    if !ALLOC_CHECK {
        return;
    }
    if let Some((w, bit)) = live_bit(p as usize) {
        rc_log(p as usize, kind as usize);
        if LIVE[w].load(core::sync::atomic::Ordering::Acquire) & bit != 0 {
            return;
        }
        // one reporter; the others would only interleave with it
        if CRASH_TAKEN.swap(true, core::sync::atomic::Ordering::AcqRel) {
            loop {
                sched_yield();
            }
        }
        rill_flush_out();
        err(b"rill: the count of a freed block was touched: ");
        err_hex(p as usize);
        err(b" (worker ");
        err_dec((*me()).index as i64);
        err(b")\n");
        free_site_of(p as usize);
        err(b"  touched by:\n");
        crash_where();
        rc_history(p as usize);
        die();
    }
}

unsafe fn alloc_complain(what: &[u8], p: usize, class: usize) -> ! {
    rill_flush_out();
    err(b"rill: ");
    err(what);
    err(b" ");
    err_hex(p);
    err(b" (class ");
    err_dec(class as i64);
    err(b", worker ");
    err_dec((*me()).index as i64);
    err(b")\n");
    crash_where();
    die()
}

/// The pool belonging to the calling thread. A single-worker program skips
/// the thread-local lookup, which is the case that has to stay fast.
#[inline(always)]
unsafe fn my_pool() -> *mut Pool {
    if rill_mt == 0 {
        &raw mut rill_pools[0]
    } else {
        &raw mut rill_pools[(*me()).index]
    }
}

#[inline]
unsafe fn pool_class(size: u64) -> Option<usize> {
    if size as usize <= POOL_MAX {
        Some((size as usize + 15) / 16 - 1)
    } else {
        None
    }
}

/// # Safety
/// Standard allocation contract; aborts on OOM.
#[no_mangle]
pub unsafe extern "C" fn rill_alloc(size: u64) -> *mut u8 {
    live_add(1);
    if let Some(class) = pool_class(size) {
        #[cfg(unix)]
        if ALLOC_QUAR && size as usize <= QUAR_MAX {
            return quar_alloc();
        }
        let pool = my_pool();
        let head = (*pool).free[class];
        if !head.is_null() {
            if ALLOC_CHECK {
                if !in_chunks(head as usize) || head as usize & 15 != 0 {
                    rill_flush_out();
                    err(b"rill: a free list holds a pointer that is not a block: ");
                    err_hex(head as usize);
                    err(b"\n");
                    free_site_of(head as usize & !15);
                    alloc_complain(b"", 0, class);
                }
                // the words after the link were painted at the free; one
                // that changed was written after the block was let go
                let words = (class + 1) * 2;
                for k in 2..words {
                    let v = *(head as *const u64).add(k);
                    if v != 0x5EE7F5EE00000000 | class as u64 {
                        rill_flush_out();
                        err(b"rill: a freed block was written after its free: block ");
                        err_hex(head as usize);
                        err(b" word ");
                        err_dec(k as i64);
                        err(b" holds ");
                        err_hex(v as usize);
                        err(b"\n  the block:");
                        for j in 0..words {
                            err(b" ");
                            err_hex(*(head as *const u64).add(j) as usize);
                        }
                        err(b"\n");
                        free_site_of(head as usize);
                        alloc_complain(b"", 0, class);
                    }
                }
            }
            if ALLOC_CHECK {
                // the link written at the free is kept once more, masked,
                // in the word after it: a count let go of by a stale holder
                // lands on the link and the two no longer agree
                let link = *(head as *const usize);
                let kept = *(head as *const usize).add(1) ^ 0x5EE7F5EE5EE7F5EE;
                if link != kept {
                    rill_flush_out();
                    err(b"rill: a freed block's count was touched after its free: block ");
                    err_hex(head as usize);
                    err(b" link was ");
                    err_hex(kept);
                    err(b", is ");
                    err_hex(link);
                    err(b"\n");
                    free_site_of(head as usize);
                    alloc_complain(b"", 0, class);
                }
                let slot = &mut rill_free_sites[(head as usize >> 4) & 0xffff];
                if slot[0] == head as usize {
                    // out again: marked, so the next free of it knows
                    slot[0] = head as usize | 1;
                }
            }
            (*pool).free[class] = *(head as *const *mut u8);
            if ALLOC_CHECK {
                live_out(head as usize, class);
                rc_log(head as usize, 5);
            }
            return head;
        }
        let bytes = (class + 1) * 16;
        if (*pool).next + bytes > (*pool).end {
            let p = mmap(core::ptr::null_mut(), POOL_CHUNK, PROT_RW, MAP_PRIVATE_ANON, -1, 0);
            if p as isize == -1 {
                out(b"rill: out of memory\n");
                die();
            }
            NCHUNKS_ALL.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            chunk_note(p as usize, POOL_CHUNK);
            (*pool).next = p as usize;
            (*pool).end = p as usize + POOL_CHUNK;
        }
        let p = (*pool).next as *mut u8;
        (*pool).next += bytes;
        if ALLOC_CHECK {
            live_out(p as usize, class);
            rc_log(p as usize, 5);
        }
        return p;
    }
    let p = malloc(size as usize);
    if p.is_null() {
        out(b"rill: out of memory\n");
        die();
    }
    p
}

/// # Safety
/// `ptr` must come from `rill_alloc` with this exact `size`, freed once.
#[no_mangle]
pub unsafe extern "C" fn rill_free_sized(ptr: *mut u8, size: u64) {
    live_add(-1);
    if let Some(class) = pool_class(size) {
        #[cfg(unix)]
        if ALLOC_QUAR && size as usize <= QUAR_MAX && !in_chunks(ptr as usize) {
            quar_free(ptr);
            return;
        }
        let pool = my_pool();
        if ALLOC_CHECK {
            if !in_chunks(ptr as usize) {
                alloc_complain(b"free of a pointer outside every chunk:", ptr as usize, class);
            }
            live_in(ptr as usize, class);
            rc_log(ptr as usize, 6);
            let words = (class + 1) * 2;
            for k in 2..words {
                *(ptr as *mut u64).add(k) = 0x5EE7F5EE00000000 | class as u64;
            }
            *(ptr as *mut usize).add(1) = (*pool).free[class] as usize ^ 0x5EE7F5EE5EE7F5EE;
            note_free(ptr as usize, (*pool).free[class] as usize);
        }
        *(ptr as *mut *mut u8) = (*pool).free[class];
        (*pool).free[class] = ptr;
        return;
    }
    if big_keep(ptr, size as usize) {
        return;
    }
    free(ptr);
}

/// Null-safe reference count increment (offset 0 of any heap object).
///
/// # Safety
/// `ptr` is null or a live heap object.
#[no_mangle]
pub unsafe extern "C" fn rill_retain(ptr: *mut u8) {
    if !ptr.is_null() {
        rc_inc(ptr as *mut i64);
    }
}

/// Null-safe closure environment release: dispatches to the environment's
/// stored drop function.
///
/// The environment is `{ i64 rc, ptr drop_fn, captures... }`, so the drop
/// function is eight bytes in — after the count, not after a pointer. Those
/// are the same distance only where a pointer is eight bytes wide, which is
/// why the offset is written in bytes rather than counted in `usize`s.
///
/// # Safety
/// `ptr` is null or a live closure environment.
#[no_mangle]
pub unsafe extern "C" fn rill_env_release(ptr: *mut u8) {
    if !ptr.is_null() {
        let drop_fn = *(ptr.add(8) as *const usize);
        let drop_fn: extern "C" fn(*mut u8) = core::mem::transmute(drop_fn);
        drop_fn(ptr);
    }
}

/// Where the program's memory is, on standard error, for a program that
/// wants to know why it weighs what it does: blocks out, the pool's chunks
/// and how much of them sits on free lists, the big blocks kept for reuse,
/// and the strand records with the stacks they have saved. Every number
/// is bytes the process holds; none of it is what `malloc` holds for
/// itself. Returns the blocks out, for a program that wants one number.
///
/// # Safety
/// Walks the pools' free lists and the strand list without locking: for a
/// program's own diagnostics, best asked while things are quiet.
#[no_mangle]
pub unsafe extern "C" fn rill_mem_report() -> i64 {
    let live = live_total();
    let mut chunks = NCHUNKS_ALL.load(core::sync::atomic::Ordering::Relaxed);
    let mut bumped = 0usize;
    let mut on_lists = 0usize;
    let mut per_class = [0usize; POOL_CLASSES];
    for i in 0..MAX_WORKERS {
        let pool = &raw const rill_pools[i];
        if (*pool).end != 0 {
            bumped += POOL_CHUNK - ((*pool).end - (*pool).next);
        }
        for class in 0..POOL_CLASSES {
            let mut p = (*pool).free[class];
            let mut n = 0usize;
            while !p.is_null() && n < 1 << 28 {
                n += 1;
                p = *(p as *const *mut u8);
            }
            per_class[class] += n;
            on_lists += n * (class + 1) * 16;
        }
    }
    if chunks == 0 && bumped > 0 {
        chunks = 1;
    }
    let mut big = 0usize;
    for i in 0..BIG_SLOTS {
        big += BIG_SIZE[i];
    }
    let mut gs = 0usize;
    let mut saved = 0usize;
    let mut g = ALL_GS.load(core::sync::atomic::Ordering::Acquire) as *mut G;
    while !g.is_null() {
        gs += 1;
        saved += (*g).saved_cap;
        g = (*g).all_next;
    }
    let mut b = [0u8; 512];
    let n = snprintf(
        b.as_mut_ptr(),
        b.len(),
        b"rill mem: %lld blocks out; pool %zu chunks of %zu MB, %zu MB handed out, %zu MB on free lists; big kept %zu MB; %zu strand records, %zu MB of saved stacks\n\0".as_ptr(),
        live,
        chunks,
        POOL_CHUNK >> 20,
        bumped >> 20,
        on_lists >> 20,
        big >> 20,
        gs,
        saved >> 20,
    );
    err(&b[..(n.max(0) as usize).min(b.len())]);
    // the big blocks kept, by size
    for i in 0..BIG_SLOTS {
        if BIG_SIZE[i] > 0 {
            let n = snprintf(b.as_mut_ptr(), b.len(), b"  big block kept: %zu KB\n\0".as_ptr(), BIG_SIZE[i] >> 10);
            err(&b[..(n.max(0) as usize).min(b.len())]);
        }
    }
    // the free lists by class, for the ones that hold anything much
    for class in 0..POOL_CLASSES {
        let bytes = per_class[class] * (class + 1) * 16;
        if bytes >= 1 << 20 {
            let n = snprintf(
                b.as_mut_ptr(),
                b.len(),
                b"  free %zu-byte blocks: %zu (%zu MB)\n\0".as_ptr(),
                (class + 1) * 16,
                per_class[class],
                bytes >> 20,
            );
            err(&b[..(n.max(0) as usize).min(b.len())]);
        }
    }
    live
}

/// Called at program exit; when RILL_DEBUG_ALLOC is set, reports the number
/// of live allocations on stderr so tests can assert it is zero.
///
/// A strand that was still running when `main` returned is named alongside it.
/// What such a strand had on its stack is owned by that stack, and finding it
/// would need a map of which words are references — which is exactly what the
/// copy-on-block scheduler is built to do without. So the count can be honest
/// about the number without the number meaning a mistake.
///
/// # Safety
/// Called once, after `rill_main` returns.
#[no_mangle]
pub unsafe extern "C" fn rill_alloc_report() {
    if getenv(b"RILL_DEBUG_ALLOC\0".as_ptr()).is_null() {
        return;
    }
    let stray = STRAY_WAKES.load(core::sync::atomic::Ordering::Relaxed);
    if stray > 0 {
        let mut b = [0u8; 96];
        let n = snprintf(
            b.as_mut_ptr(),
            95,
            b"poller events with no owner, dropped: %lld\n\0".as_ptr(),
            stray as i64,
        );
        write(2, b.as_ptr(), n.clamp(0, 95) as usize);
    }
    let mut buf = [0u8; 128];
    let n = if ABANDONED == 0 {
        snprintf(buf.as_mut_ptr(), 127, b"live allocations: %lld\n\0".as_ptr(), live_total())
    } else {
        snprintf(
            buf.as_mut_ptr(),
            127,
            b"live allocations: %lld (%lld strand(s) were still running at exit)\n\0".as_ptr(),
            live_total(),
            ABANDONED,
        )
    };
    let n = n.clamp(0, 127) as usize;
    write(2, buf.as_ptr(), n);
}

/// # Safety
/// Both must be live string objects.
#[no_mangle]
pub unsafe extern "C" fn rill_str_eq(a: *const u8, b: *const u8) -> i8 {
    let (la, lb) = (str_len_of(a), str_len_of(b));
    if la != lb {
        return 0;
    }
    let a = core::slice::from_raw_parts(str_bytes(a), la);
    let b = core::slice::from_raw_parts(str_bytes(b), lb);
    (a == b) as i8
}

// ---- concurrency: strands, channels and the scheduler ------------------
//
// Go's model. Strands are green threads multiplexed over one or more OS
// worker threads; channels are unbuffered rendezvous points.
//
// Parallelism is opt-in per run: `RILL_THREADS=N` starts N workers. The
// default of one worker keeps reference counting non-atomic, which is what
// makes single-threaded Rill as fast as it is; with more than one worker the
// runtime flips `rill_mt` before any thread starts and every count becomes
// atomic from then on. Programs that never set the variable pay one
// perfectly-predicted branch.
//
// Stacks use a copy-on-block model: every strand on a worker runs on that
// worker's single guarded execution stack, and parking copies out only the
// bytes actually in use (a few hundred, typically) to be copied back to the
// same address on resume. Interior pointers therefore stay valid without
// compiler stack maps — and, because the address is per worker, a strand
// that has already run is pinned to its worker. Work stealing moves only
// strands that have not started yet, which is where the imbalance is.

const EXEC_STACK_SIZE: usize = 8 * 1024 * 1024;
const MAIN_STACK_SIZE: usize = 32 * 1024 * 1024;
const PAGE: usize = 16 * 1024;
const MAX_WORKERS: usize = 32;

const PROT_NONE: i32 = 0;
const PROT_RW: i32 = 3;

/// "Private, and backed by nothing" — one idea, two numbers. `MAP_PRIVATE` is
/// 0x02 everywhere; the anonymous bit is 0x1000 on the BSDs and 0x20 on Linux.
/// Using the BSD one on Linux does not fail loudly: 0x1000 there is
/// `MAP_EXECUTABLE`, so the call asks for an executable file mapping of file
/// descriptor -1 and comes back with `MAP_FAILED`, which surfaces as "cannot
/// map strand stack" the first time a program spawns anything.
#[cfg(not(target_os = "linux"))]
const MAP_PRIVATE_ANON: i32 = 0x0002 | 0x1000;
#[cfg(target_os = "linux")]
const MAP_PRIVATE_ANON: i32 = 0x0002 | 0x0020;

#[cfg(unix)]
extern "C" {
    fn mmap(addr: *mut u8, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> *mut u8;
    fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32;
    fn munmap(addr: *mut u8, len: usize) -> i32;
}

/// The same three things Windows calls something else. `system` rather than
/// `C`, which on this architecture is the same convention but says what these
/// are: the operating system's own, not the C library's.
#[cfg(windows)]
extern "system" {
    fn VirtualAlloc(addr: *mut u8, len: usize, kind: u32, protect: u32) -> *mut u8;
    fn VirtualProtect(addr: *mut u8, len: usize, protect: u32, was: *mut u32) -> i32;
    fn VirtualFree(addr: *mut u8, len: usize, kind: u32) -> i32;
}

#[cfg(windows)]
const MEM_COMMIT_RESERVE: u32 = 0x1000 | 0x2000;
#[cfg(windows)]
const MEM_RELEASE: u32 = 0x8000;
#[cfg(windows)]
const PAGE_READWRITE: u32 = 0x04;
#[cfg(windows)]
const PAGE_NOACCESS: u32 = 0x01;

/// ---- the same nine things, said in Windows ---------------------------------
///
/// Every one of these is named after its POSIX counterpart and takes the same
/// arguments, so that nothing above this line has to know which operating
/// system it is on. The bodies are Win32; the names are the ones the runtime
/// already calls.
#[cfg(windows)]
extern "system" {
    fn CreateThread(
        attrs: *const u8,
        stack: usize,
        start: unsafe extern "system" fn(*mut u8) -> u32,
        arg: *mut u8,
        flags: u32,
        id: *mut u32,
    ) -> *mut u8;
    fn TlsAlloc() -> u32;
    fn TlsGetValue(slot: u32) -> *mut u8;
    fn TlsSetValue(slot: u32, value: *const u8) -> i32;
    fn InitializeSRWLock(lock: *mut usize);
    fn AcquireSRWLockExclusive(lock: *mut usize);
    fn ReleaseSRWLockExclusive(lock: *mut usize);
    fn SwitchToThread() -> i32;
}

/// Wasm's memory only ever grows and nothing is given back, which is exactly
/// how the pool uses this call: chunks are bump-allocated and never unmapped.
/// So `malloc` is not a stand-in for `mmap` here — it is the same thing. The
/// failure value is the one `mmap` would have given, so the caller's test
/// still reads.
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn mmap(_addr: *mut u8, len: usize, _prot: i32, _flags: i32, _fd: i32, _off: i64) -> *mut u8 {
    let p = malloc(len);
    if p.is_null() { usize::MAX as *mut u8 } else { p }
}

/// The pool allocator asks for memory the way a stack does; both go through
/// the same call, and the failure value is the one `mmap` would have given so
/// that the caller's test still reads.
#[cfg(windows)]
unsafe fn mmap(_addr: *mut u8, len: usize, _prot: i32, _flags: i32, _fd: i32, _off: i64) -> *mut u8 {
    let p = VirtualAlloc(core::ptr::null_mut(), len, MEM_COMMIT_RESERVE, PAGE_READWRITE);
    if p.is_null() { usize::MAX as *mut u8 } else { p }
}

#[cfg(windows)]
unsafe fn mprotect(addr: *mut u8, len: usize, prot: i32) -> i32 {
    let mut was: u32 = 0;
    let want = if prot == PROT_NONE { PAGE_NOACCESS } else { PAGE_READWRITE };
    if VirtualProtect(addr, len, want, &raw mut was) == 0 { -1 } else { 0 }
}

/// A `SRWLOCK` is one pointer wide and starts life as zero, which is what the
/// first word of the struct already is — so initialising one is saying so.
#[cfg(windows)]
unsafe fn pthread_mutex_init(m: *mut PthreadMutex, _attr: *const u8) -> i32 {
    InitializeSRWLock(m as *mut usize);
    0
}

#[cfg(windows)]
unsafe fn pthread_mutex_lock(m: *mut PthreadMutex) -> i32 {
    AcquireSRWLockExclusive(m as *mut usize);
    0
}

#[cfg(windows)]
unsafe fn pthread_mutex_unlock(m: *mut PthreadMutex) -> i32 {
    ReleaseSRWLockExclusive(m as *mut usize);
    0
}

/// The worker's entry takes a pointer and gives one back; Windows wants one
/// that gives back a number. The pointer is thrown away either way, and on
/// this architecture the two are called identically, so the function is
/// handed over as it is.
#[cfg(windows)]
unsafe fn pthread_create(
    thread: *mut usize,
    _attr: *const u8,
    start: extern "C" fn(*mut u8) -> *mut u8,
    arg: *mut u8,
) -> i32 {
    let entry: unsafe extern "system" fn(*mut u8) -> u32 = core::mem::transmute(start);
    let handle = CreateThread(core::ptr::null(), 0, entry, arg, 0, core::ptr::null_mut());
    if handle.is_null() {
        return -1;
    }
    *thread = handle as usize;
    0
}

#[cfg(windows)]
unsafe fn pthread_key_create(key: *mut usize, _destructor: *const u8) -> i32 {
    let slot = TlsAlloc();
    if slot == u32::MAX {
        return -1;
    }
    *key = slot as usize;
    0
}

#[cfg(windows)]
unsafe fn pthread_getspecific(key: usize) -> *mut u8 {
    TlsGetValue(key as u32)
}

#[cfg(windows)]
unsafe fn pthread_setspecific(key: usize, value: *const u8) -> i32 {
    if TlsSetValue(key as u32, value) == 0 { -1 } else { 0 }
}

#[cfg(windows)]
unsafe fn sched_yield() -> i32 {
    SwitchToThread()
}

/// Wasm has one thread and no way to make another, so the primitives the
/// scheduler reaches for are all the same answer: there is nobody else. A
/// lock is held by whoever asked, a thread-local is the only local there is,
/// and starting a worker fails. Nothing here is ever called — a program that
/// spawns is refused at compile time — but the scheduler still has to link.
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn pthread_mutex_init(_m: *mut PthreadMutex, _attr: *const u8) -> i32 {
    0
}

#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn pthread_mutex_lock(_m: *mut PthreadMutex) -> i32 {
    0
}

#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn pthread_mutex_unlock(_m: *mut PthreadMutex) -> i32 {
    0
}

#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn pthread_create(
    _thread: *mut usize,
    _attr: *const u8,
    _start: extern "C" fn(*mut u8) -> *mut u8,
    _arg: *mut u8,
) -> i32 {
    -1
}

#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn pthread_key_create(_key: *mut usize, _destructor: *const u8) -> i32 {
    0
}

#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn pthread_setspecific(_key: usize, _value: *const u8) -> i32 {
    0
}

#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn pthread_getspecific(_key: usize) -> *mut u8 {
    core::ptr::null_mut()
}

/// There is no other thread to yield to.
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn sched_yield() -> i32 {
    0
}

/// How long a strand may hold a worker while another waits for one. Ten
/// milliseconds is what a scheduler of this kind usually settles on: long
/// enough that a strand doing ordinary work is never interrupted for nothing,
/// short enough that nobody waits noticeably for their turn.
const PREEMPT_SLICE_NANOS: i64 = 10_000_000;

/// How much longer to wait between looks when no strand is waiting for a turn.
const PREEMPT_IDLE_FACTOR: i64 = 10;

#[cfg(unix)]
#[repr(C)]
struct Timespec {
    secs: i64,
    nanos: i64,
}

#[cfg(unix)]
extern "C" {
    fn nanosleep(req: *const Timespec, rem: *mut Timespec) -> i32;
}

#[cfg(unix)]
unsafe fn sleep_nanos(nanos: i64) {
    let t = Timespec { secs: nanos / 1_000_000_000, nanos: nanos % 1_000_000_000 };
    nanosleep(&t, core::ptr::null_mut());
}

#[cfg(windows)]
extern "system" {
    fn Sleep(millis: u32);
}

#[cfg(windows)]
unsafe fn sleep_nanos(nanos: i64) {
    Sleep((nanos / 1_000_000).max(1) as u32);
}

/// Never called: the timer thread is started with the scheduler, and a wasm
/// build that would reach the scheduler is refused at compile time.
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn sleep_nanos(_nanos: i64) {}

#[cfg(unix)]
extern "C" {
    fn pthread_mutex_init(m: *mut PthreadMutex, attr: *const u8) -> i32;
    fn pthread_create(
        thread: *mut usize,
        attr: *const u8,
        start: extern "C" fn(*mut u8) -> *mut u8,
        arg: *mut u8,
    ) -> i32;
    fn pthread_mutex_lock(m: *mut PthreadMutex) -> i32;
    fn pthread_mutex_unlock(m: *mut PthreadMutex) -> i32;
    fn pthread_key_create(key: *mut usize, destructor: *const u8) -> i32;
    fn pthread_setspecific(key: usize, value: *const u8) -> i32;
    fn pthread_getspecific(key: usize) -> *mut u8;
    fn pthread_self() -> usize;
    fn sched_yield() -> i32;
}

/// Emitted by the compiler, on every target: 1 when the binary was built with
/// `--parallel`. It has nothing to do with the operating system and is not
/// gated with the things that do.
extern "C" {
    static rill_parallel_build: i8;
}

/// macOS `pthread_mutex_t`, statically initialized the way the C macro does.
#[repr(C)]
struct PthreadMutex {
    sig: i64,
    opaque: [u8; 56],
}

/// Maps a stack with a guard page below it; returns the mapping base.
#[cfg(unix)]
unsafe fn alloc_stack(usable: usize) -> *mut u8 {
    let total = usable + PAGE;
    let p = mmap(core::ptr::null_mut(), total, PROT_RW, MAP_PRIVATE_ANON, -1, 0);
    if p as isize == -1 {
        out(b"rill: cannot map strand stack\n");
        die();
    }
    mprotect(p, PAGE, PROT_NONE);
    p
}

/// The same thing on Windows: one reservation, and its first page turned to
/// nothing so a stack that runs off its end faults instead of quietly writing
/// over whatever was mapped below it.
#[cfg(windows)]
unsafe fn alloc_stack(usable: usize) -> *mut u8 {
    let total = usable + PAGE;
    let p = VirtualAlloc(core::ptr::null_mut(), total, MEM_COMMIT_RESERVE, PAGE_READWRITE);
    if p.is_null() {
        out(b"rill: cannot map strand stack\n");
        die();
    }
    let mut was: u32 = 0;
    VirtualProtect(p, PAGE, PAGE_NOACCESS, &raw mut was);
    p
}

/// Wasm's call stack is the engine's, not memory this program can address, so
/// there is no such thing as a second one to run a strand on. Reaching here
/// means the compile-time refusal was not applied.
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn alloc_stack(_usable: usize) -> *mut u8 {
    out(b"rill: strands are not available on wasm\n");
    die();
}

/// Giving one back. Nothing does yet — a worker's stack lives as long as the
/// program — but the pair is written out because a half of one is how a leak
/// starts.
#[cfg(unix)]
#[allow(dead_code)]
unsafe fn free_stack(p: *mut u8, usable: usize) {
    munmap(p, usable + PAGE);
}

#[cfg(windows)]
#[allow(dead_code)]
unsafe fn free_stack(p: *mut u8, _usable: usize) {
    VirtualFree(p, 0, MEM_RELEASE);
}

#[cfg(any(target_family = "wasm", target_os = "none"))]
#[allow(dead_code)]
unsafe fn free_stack(_p: *mut u8, _usable: usize) {}

/// Saved callee-saved state: x19..x28, x29 (fp), x30 (lr), sp, d8..d15.
#[repr(C)]
struct Ctx {
    regs: [u64; 21],
}

/// A strand.
#[repr(C)]
struct G {
    ctx: Ctx,
    /// Stack bytes copied out while parked (spawned strands only).
    saved: *mut u8,
    saved_len: usize,
    saved_cap: usize,
    code: extern "C" fn(*mut u8),
    env: *mut u8,
    /// Intrusive link: a run queue, a channel wait queue, or the free list.
    next: *mut G,
    /// Channel transfer slot (two words fit every Rill value).
    item: [u64; 2],
    /// Where `chan_wait` leaves the value it took, for the `chan_taken` that
    /// follows it. One strand's, so nothing has to guard it: a strand cannot
    /// be between its own two calls twice.
    taken: [u64; 2],
    dead: bool,
    /// The main strand runs on its own stack and never copies.
    is_main: bool,
    /// False until the strand first runs; only these may be stolen.
    started: bool,
    /// The worker whose execution stack this strand's saved bytes belong to.
    home: usize,
    /// The `select` case that claimed this strand, or -1 while it is still
    /// waiting. Also marks plain sends and receives as claimed.
    selected: i64,
    /// True from the moment this strand hands a descriptor or a deadline to
    /// the poller until the poller hands it back.
    ///
    /// The poller used to go by `selected < 0`, which a strand parked on a
    /// channel also has: an event that arrived for a strand no longer waiting
    /// on it — a registration outliving the wait that made it — would then
    /// wake a strand sitting in a channel's queue, and `recv` would return the
    /// two zero words its waiter was initialised with. In Rill that is a null
    /// string, and the first thing anybody does with a received line is ask
    /// its length. This says plainly which of the two waits the strand is in,
    /// so an event that no longer has an owner is dropped instead of being
    /// delivered to whoever holds the record now.
    on_io: bool,
    /// What the strand is parked on when `on_io` is set: a descriptor plus
    /// one, or 0 for a timer alone. A descriptor's registration in the poller
    /// now outlives the wait that made it, so an event can arrive for a
    /// strand that has moved on to another descriptor; this is how the poller
    /// tells that event from the one being waited for.
    wait_ident: usize,
    /// Queue node for a plain send or receive, which never needs more than one.
    waiter: Waiter,
    /// A host strand's wake flag (see `rill_host_enter`); 0 on any other.
    host_woken: i64,
    /// Every strand record ever allocated, on one list. `next` cannot be used
    /// for this — it is already the link for whichever queue the strand is
    /// sitting in — and nothing else knows where the strands are: a parked one
    /// is in a channel's wait queue, a fresh one in a run queue, and neither
    /// is reachable from the other. See `sweep_abandoned`.
    all_next: *mut G,
}

/// Head of that list. Strand records are recycled through their worker's free
/// list and never handed back, so this only grows to the most strands that
/// ever existed at once.
static ALL_GS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Strands that have been started and have not finished. Kept so that the
/// shutdown sweep can tell, without looking, that there is nothing to look
/// for — which is the usual answer, and worth having cheaply: walking the
/// list is a cache miss per strand, and a program like `ring` makes a hundred
/// thousand of them and leaves none of them running.
static LIVE_STRANDS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Puts a newly allocated strand record on the list of all of them.
unsafe fn register_g(g: *mut G) {
    use core::sync::atomic::Ordering;
    let mut head = ALL_GS.load(Ordering::Relaxed);
    loop {
        (*g).all_next = head as *mut G;
        match ALL_GS.compare_exchange_weak(head, g as usize, Ordering::Release, Ordering::Relaxed) {
            Ok(_) => return,
            Err(h) => head = h,
        }
    }
}

struct Queue {
    head: *mut G,
    tail: *mut G,
}

impl Queue {
    const fn new() -> Queue {
        Queue { head: core::ptr::null_mut(), tail: core::ptr::null_mut() }
    }

    unsafe fn push(&mut self, g: *mut G) {
        (*g).next = core::ptr::null_mut();
        if self.tail.is_null() {
            self.head = g;
        } else {
            (*self.tail).next = g;
        }
        self.tail = g;
    }

    unsafe fn pop(&mut self) -> *mut G {
        let g = self.head;
        if !g.is_null() {
            self.head = (*g).next;
            if self.head.is_null() {
                self.tail = core::ptr::null_mut();
            }
        }
        g
    }

    /// Removes the first strand that has never run, for work stealing.
    unsafe fn steal_fresh(&mut self) -> *mut G {
        let mut prev: *mut G = core::ptr::null_mut();
        let mut cur = self.head;
        while !cur.is_null() {
            if !(*cur).started {
                if prev.is_null() {
                    self.head = (*cur).next;
                } else {
                    (*prev).next = (*cur).next;
                }
                if self.tail == cur {
                    self.tail = prev;
                }
                (*cur).next = core::ptr::null_mut();
                return cur;
            }
            prev = cur;
            cur = (*cur).next;
        }
        core::ptr::null_mut()
    }
}

/// One OS thread's share of the scheduler. Each worker locks only its own run
/// queue, so workers that are simply running strands never contend; the
/// channel lock is separate and is always taken first when both are needed.
struct Worker {
    sched_ctx: Ctx,
    current: *mut G,
    runq: Queue,
    /// Guards `runq` alone.
    lock: PthreadMutex,
    /// Finished strand records, reused by this worker only.
    free: *mut G,
    exec_top: usize,
    index: usize,
    /// This worker's netpoller. One per worker, because a strand that has
    /// run is tied to its worker's stack and only that worker can resume it:
    /// the readiness event has to arrive where the strand lives.
    kq: core::sync::atomic::AtomicI32,
    /// How to interrupt this worker's wait. kqueue can be poked directly with
    /// a user event, so on BSD this stays -1; epoll has no such thing and needs
    /// a real descriptor in the set to write to, which is what an eventfd is
    /// for. The difference is small enough to carry in one field rather than in
    /// two shapes of `Worker`.
    wake_fd: core::sync::atomic::AtomicI32,
    /// Set while this worker is asleep inside the poller, so that another
    /// worker only pays for a wakeup syscall when there is really someone to
    /// wake.
    asleep: core::sync::atomic::AtomicBool,
}

const WORKER_INIT: Worker = Worker {
    sched_ctx: Ctx { regs: [0; 21] },
    current: core::ptr::null_mut(),
    runq: Queue::new(),
    // Zero, not the static mutex initialiser: a non-zero byte anywhere in
    // this table would put all of it in `__DATA` and so in the executable
    // file. `rill_rt_run` initialises the workers it actually uses.
    lock: PthreadMutex { sig: 0, opaque: [0; 56] },
    free: core::ptr::null_mut(),
    exec_top: 0,
    index: 0,
    kq: core::sync::atomic::AtomicI32::new(0),
    wake_fd: core::sync::atomic::AtomicI32::new(0),
    asleep: core::sync::atomic::AtomicBool::new(false),
};

static mut WORKERS: [Worker; MAX_WORKERS] = [WORKER_INIT; MAX_WORKERS];
/// Assigned by `rill_rt_run` before anything reads it; zero here keeps the
/// table out of the binary.
static mut NWORKERS: usize = 0;

/// How many workers this run has — `RILL_THREADS` as the runtime took it,
/// 1 when it was not set or the build was not `--parallel`. What a library
/// asks before deciding how many strands a big piece of work is worth.
#[no_mangle]
pub unsafe extern "C" fn rill_workers() -> i64 {
    let n = NWORKERS;
    if n == 0 { 1 } else { n as i64 }
}
/// Strands currently executing on some worker, and strands sitting in
/// some run queue. Both are maintained under the scheduler lock, so seeing
/// them at zero together proves nothing can ever become runnable again —
/// unlike "every worker is idle", which is only ever a stale snapshot.
static RUNNING: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
static QUEUED: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
/// Set when the main strand dies, and read by every worker at the top of its
/// loop. Atomic because the shutdown sweep's safety rests on it: a worker that
/// has seen this will take no further strand, which is what makes "every
/// worker has left its loop" mean "nobody is looking at a strand any more".
static STOP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Workers 1..NWORKERS that have left `worker_loop` for good. Worker 0 is the
/// thread doing the counting, so it is never included.
static WORKERS_EXITED: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
static mut MAIN_G: *mut G = core::ptr::null_mut();
/// Where the main strand's stack ends (its highest address), for telling a
/// fault in its guard page from any other.
static mut MAIN_STACK_TOP: usize = 0;
static mut TLS_KEY: usize = 0;
/// Guards every channel's queues and the claiming of waiting strands.
static mut CHAN_LOCK: PthreadMutex = PthreadMutex { sig: 0, opaque: [0; 56] };

/// Set once, before any worker thread exists, when the program asks for more
/// than one. Generated code reads it too, to decide between plain and atomic
/// refcount updates.
#[no_mangle]
pub static mut rill_mt: i8 = 0;

#[inline(always)]
unsafe fn lock() {
    if rill_mt != 0 {
        pthread_mutex_lock(&raw mut CHAN_LOCK);
    }
}

#[inline(always)]
unsafe fn unlock() {
    if rill_mt != 0 {
        pthread_mutex_unlock(&raw mut CHAN_LOCK);
    }
}

#[inline(always)]
unsafe fn lock_runq(w: *mut Worker) {
    if rill_mt != 0 {
        pthread_mutex_lock(&raw mut (*w).lock);
    }
}

#[inline(always)]
unsafe fn unlock_runq(w: *mut Worker) {
    if rill_mt != 0 {
        pthread_mutex_unlock(&raw mut (*w).lock);
    }
}

#[inline(always)]
fn bump(counter: &core::sync::atomic::AtomicUsize, delta: isize) {
    if delta >= 0 {
        counter.fetch_add(delta as usize, core::sync::atomic::Ordering::AcqRel);
    } else {
        counter.fetch_sub((-delta) as usize, core::sync::atomic::Ordering::AcqRel);
    }
}

#[inline(always)]
unsafe fn me() -> *mut Worker {
    if rill_mt == 0 {
        &raw mut WORKERS[0]
    } else {
        pthread_getspecific(TLS_KEY) as *mut Worker
    }
}

/// The context switch: saves callee-saved state into `old`, restores `new`,
/// and returns on the new context's stack.
///
/// Two of them, because the registers a function may keep and the registers
/// it must hand back are the one thing every architecture disagrees about.
/// Both write the same twenty-one slots, and both use slot 11 for where to
/// resume and slot 12 for the stack: on ARM that is what the link register
/// and the stack pointer already are, and on x86-64 the return address is
/// lifted off the stack to make it look the same.
#[cfg(target_arch = "aarch64")]
#[unsafe(naked)]
unsafe extern "C" fn ctx_switch(_old: *mut Ctx, _new: *const Ctx) {
    core::arch::naked_asm!(
        "stp x19, x20, [x0, #0]",
        "stp x21, x22, [x0, #16]",
        "stp x23, x24, [x0, #32]",
        "stp x25, x26, [x0, #48]",
        "stp x27, x28, [x0, #64]",
        "stp x29, x30, [x0, #80]",
        "mov x9, sp",
        "str x9, [x0, #96]",
        "stp d8, d9, [x0, #104]",
        "stp d10, d11, [x0, #120]",
        "stp d12, d13, [x0, #136]",
        "stp d14, d15, [x0, #152]",
        "ldp x19, x20, [x1, #0]",
        "ldp x21, x22, [x1, #16]",
        "ldp x23, x24, [x1, #32]",
        "ldp x25, x26, [x1, #48]",
        "ldp x27, x28, [x1, #64]",
        "ldr x9, [x1, #96]",
        "mov sp, x9",
        "ldp d8, d9, [x1, #104]",
        "ldp d10, d11, [x1, #120]",
        "ldp d12, d13, [x1, #136]",
        "ldp d14, d15, [x1, #152]",
        "ldp x29, x30, [x1, #80]",
        "ret",
    )
}

/// x86-64, System V: the arguments arrive in rdi and rsi, and rbx, rbp and
/// r12 through r15 are what a function must give back. There is no link
/// register, so the return address is taken off the top of the stack into
/// slot 11 and the switch ends by jumping to the new one's.
#[cfg(all(target_arch = "x86_64", not(windows)))]
#[unsafe(naked)]
unsafe extern "C" fn ctx_switch(_old: *mut Ctx, _new: *const Ctx) {
    core::arch::naked_asm!(
        "mov [rdi + 0], rbx",
        "mov [rdi + 8], rbp",
        "mov [rdi + 16], r12",
        "mov [rdi + 24], r13",
        "mov [rdi + 32], r14",
        "mov [rdi + 40], r15",
        "mov rax, [rsp]",
        "mov [rdi + 88], rax",
        "lea rax, [rsp + 8]",
        "mov [rdi + 96], rax",
        "mov rbx, [rsi + 0]",
        "mov rbp, [rsi + 8]",
        "mov r12, [rsi + 16]",
        "mov r13, [rsi + 24]",
        "mov r14, [rsi + 32]",
        "mov r15, [rsi + 40]",
        "mov rsp, [rsi + 96]",
        "jmp qword ptr [rsi + 88]",
    )
}

/// x86-64 on Windows: the arguments arrive in rcx and rdx instead, and rsi
/// and rdi join the registers that have to survive a call — which is why
/// they are saved here and not above.
///
/// The vector registers xmm6 to xmm15 are callee-saved on Windows as well and
/// are *not* saved here yet; nothing in the runtime uses them, but code
/// compiled through this switch one day might.
#[cfg(all(target_arch = "x86_64", windows))]
#[unsafe(naked)]
unsafe extern "C" fn ctx_switch(_old: *mut Ctx, _new: *const Ctx) {
    core::arch::naked_asm!(
        "mov [rcx + 0], rbx",
        "mov [rcx + 8], rbp",
        "mov [rcx + 16], r12",
        "mov [rcx + 24], r13",
        "mov [rcx + 32], r14",
        "mov [rcx + 40], r15",
        "mov [rcx + 104], rsi",
        "mov [rcx + 112], rdi",
        "mov rax, [rsp]",
        "mov [rcx + 88], rax",
        "lea rax, [rsp + 8]",
        "mov [rcx + 96], rax",
        "mov rbx, [rdx + 0]",
        "mov rbp, [rdx + 8]",
        "mov r12, [rdx + 16]",
        "mov r13, [rdx + 24]",
        "mov r14, [rdx + 32]",
        "mov r15, [rdx + 40]",
        "mov rsi, [rdx + 104]",
        "mov rdi, [rdx + 112]",
        "mov rsp, [rdx + 96]",
        "jmp qword ptr [rdx + 88]",
    )
}

/// The one architecture where this cannot be written. A context switch means
/// saving the registers and the stack pointer, and wasm exposes neither: the
/// call stack belongs to the engine, and the only memory a module can address
/// is its own linear memory, which the stack is not in. The copy-on-block
/// model the scheduler is built around therefore has nothing to copy. Getting
/// strands onto wasm is asyncify or JSPI, not a port of this function.
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe extern "C" fn ctx_switch(_old: *mut Ctx, _new: *const Ctx) {
    out(b"rill: strands are not available on wasm\n");
    die();
}

/// First frame of every strand: run the body, release the environment,
/// then hand control back to the scheduler for good.
unsafe extern "C" fn g_entry() {
    let w = me();
    let g = (*w).current;
    ((*g).code)((*g).env);
    rill_env_release((*g).env);
    (*g).dead = true;
    ctx_switch(&mut (*g).ctx, &(*w).sched_ctx);
    core::hint::unreachable_unchecked()
}

/// Where a strand's stack pointer starts, given the top of the stack it is to
/// run on.
///
/// A strand is entered with a jump, not a call, and on x86-64 that is an eight
/// byte difference the compiler is entitled to assume. The System V and
/// Windows conventions both require the stack to be sixteen-byte aligned at a
/// `call`; the call then pushes an eight byte return address, so every
/// function begins life with a stack pointer eight past an alignment boundary
/// and its prologue is built around that. Entering by jump with a
/// sixteen-aligned pointer leaves everything downstream misaligned by eight,
/// and the first aligned SSE store — which is how a compiler spills a pair of
/// doubles, or how `memcpy` moves sixteen bytes at a time — takes a general
/// protection fault.
///
/// AArch64 has no return address on the stack and requires sixteen-byte
/// alignment at all times, so there the top is exactly right. Which is why
/// this was invisible on an Apple Silicon Mac and immediate on anything else:
/// the strand would run, block, resume, and die on the first line that
/// happened to touch a vector register.
#[cfg(target_arch = "x86_64")]
unsafe fn stack_start(top: usize) -> u64 {
    ((top & !15) - 8) as u64
}

#[cfg(not(target_arch = "x86_64"))]
unsafe fn stack_start(top: usize) -> u64 {
    (top & !15) as u64
}

/// Resets a strand record for (re)use.
unsafe fn init_g(g: *mut G, code: extern "C" fn(*mut u8), env: *mut u8) {
    (*g).ctx = Ctx { regs: [0; 21] };
    (*g).saved_len = 0;
    (*g).code = code;
    (*g).env = env;
    (*g).next = core::ptr::null_mut();
    (*g).item = [0; 2];
    (*g).dead = false;
    (*g).started = false;
    (*g).home = 0;
    (*g).selected = -1;
    (*g).on_io = false;
    (*g).host_woken = 0;
    (*g).waiter = WAITER_INIT;
    // Slot 11 is where the switch resumes: the link register on ARM, and
    // the address it jumps to on x86-64.
    (*g).ctx.regs[11] = g_entry as usize as u64;
}

/// A spawned strand: a small record from the free list or freshly
/// allocated. No stack allocation at all — it borrows its worker's.
unsafe fn new_spawned_g(w: *mut Worker, code: extern "C" fn(*mut u8), env: *mut u8) -> *mut G {
    let g = if !(*w).free.is_null() {
        let g = (*w).free;
        (*w).free = (*g).next;
        g
    } else {
        let g = malloc(core::mem::size_of::<G>()) as *mut G;
        if g.is_null() {
            out(b"rill: out of memory spawning strand\n");
            die();
        }
        (*g).saved = core::ptr::null_mut();
        (*g).saved_cap = 0;
        register_g(g);
        g
    };
    (*g).is_main = false;
    init_g(g, code, env);
    g
}

/// The main strand: its own guarded, lazily-committed stack, never copied.
unsafe fn new_main_g(code: extern "C" fn(*mut u8)) -> *mut G {
    let g = malloc(core::mem::size_of::<G>()) as *mut G;
    if g.is_null() {
        out(b"rill: out of memory spawning strand\n");
        die();
    }
    let total = MAIN_STACK_SIZE + PAGE;
    let stack = alloc_stack(MAIN_STACK_SIZE);
    MAIN_STACK_TOP = stack as usize + total;
    (*g).saved = core::ptr::null_mut();
    (*g).saved_cap = 0;
    (*g).is_main = true;
    init_g(g, code, core::ptr::null_mut());
    (*g).started = true;
    (*g).ctx.regs[12] = stack_start(stack as usize + total);
    g
}

unsafe fn restore_stack(g: *mut G, exec_top: usize) {
    if (*g).saved_len > 0 {
        core::ptr::copy_nonoverlapping(
            (*g).saved,
            (exec_top - (*g).saved_len) as *mut u8,
            (*g).saved_len,
        );
        (*g).saved_len = 0;
    }
}

unsafe fn save_stack(g: *mut G, exec_top: usize) {
    let sp = (*g).ctx.regs[12] as usize;
    let len = exec_top - sp;
    if len > (*g).saved_cap {
        if !(*g).saved.is_null() {
            free((*g).saved);
        }
        let cap = len.next_power_of_two();
        (*g).saved = malloc(cap);
        if (*g).saved.is_null() {
            out(b"rill: out of memory parking strand\n");
            die();
        }
        (*g).saved_cap = cap;
    }
    core::ptr::copy_nonoverlapping(sp as *const u8, (*g).saved, len);
    (*g).saved_len = len;
}

/// Makes a strand runnable again. A strand that has already run is
/// pinned to the worker whose stack holds its frames; a fresh one goes to the
/// waker's own queue, where any idle worker may steal it.
unsafe fn make_ready(g: *mut G) {
    // A host strand is never queued: it is pumping the run queues from its
    // own stack, and looks for its flag between strands.
    if is_host(g) {
        core::sync::atomic::AtomicI64::from_ptr(&raw mut (*g).host_woken)
            .store(HOST_WAKE, core::sync::atomic::Ordering::Release);
        return;
    }
    let w = if (*g).started { &raw mut WORKERS[(*g).home] } else { me() };
    lock_runq(w);
    (*w).runq.push(g);
    bump(&QUEUED, 1);
    unlock_runq(w);
    if rill_mt != 0 && w != me() {
        // The worker this belongs to may be asleep waiting on the network.
        wake_worker(w);
    }
}

/// Parks the current strand; the scheduler saves its stack once control
/// returns. Callers must have released the scheduler lock.
unsafe fn block_current() {
    // Nothing a program has written waits on the program while it does.
    #[cfg(not(any(target_os = "wasi", target_os = "none")))]
    net::flush_pending();
    let w = me();
    let g = (*w).current;
    if is_host(g) {
        host_pump(w, g);
        return;
    }
    ctx_switch(&mut (*g).ctx, &(*w).sched_ctx);
}

// ---- host mode: a shared library's scheduler ------------------------------
//
// A library has no `main` strand: the host's thread calls the exports, on
// its own stack, and that stack cannot be parked. So in a library the host
// thread is worker 0 and its call is the *host strand* — a strand record
// that is never queued and never copied — and whenever it waits on a channel
// it runs the other strands itself, from its own stack, on worker 0's
// execution stack, until what it waits for has arrived. Workers 1..N are
// threads as in a program, started when `RILL_THREADS` asks for them; a
// strand the host spawns is stolen by them or run by the host's pump. Only
// channels wake the host (a socket or a timer would not), which is what a
// library that splits a piece of work across strands needs.

/// The first host strand, or null in a program: what says the library is
/// in host mode at all. Every host thread has a host strand of its own,
/// kept in its worker; see `rill_host_enter`.
static mut HOST_G: *mut G = core::ptr::null_mut();

/// How many host threads have come in; the second and later take worker
/// slots after the strand workers.
static HOST_THREADS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Export calls in flight, over every host thread; see `rill_host_enter`.
static HOST_ACTIVE: core::sync::atomic::AtomicI64 = core::sync::atomic::AtomicI64::new(0);

/// Whether a second host thread has ever called in. From then on counts
/// stay atomic: `rill_mt_off` is a no-op.
static HOST_MULTI: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether a strand is a host strand: never queued, never copied, woken
/// through its own flag.
unsafe fn is_host(g: *mut G) -> bool {
    !g.is_null() && (*g).is_main && !HOST_G.is_null() && (*g).host_woken != 0
}

/// A host strand's `host_woken` word: 1 while waiting, 2 once woken. It is
/// 1 whenever the strand exists (so `is_host` can tell it from a program's
/// main strand), and the pump reads 2 as its wake.
const HOST_IDLE: i64 = 1;
const HOST_WAKE: i64 = 2;

unsafe fn new_host_g() -> *mut G {
    let g = malloc(core::mem::size_of::<G>()) as *mut G;
    if g.is_null() {
        out(b"rill: out of memory starting a host strand\n");
        die();
    }
    (*g).saved = core::ptr::null_mut();
    (*g).saved_cap = 0;
    (*g).is_main = true;
    init_g(g, host_nothing, core::ptr::null_mut());
    (*g).started = true;
    (*g).host_woken = HOST_IDLE;
    g
}

extern "C" fn host_nothing(_env: *mut u8) {}

/// Called on the way into every export of a library that has strands: the
/// thread calling in becomes a host worker if it is not one yet — its own
/// execution stack for pumping, pools, poller and host strand. The first
/// host thread is worker 0, made by `rill_rt_host`; a later one takes a
/// slot past the strand workers, which only `RILL_THREADS` of two or more
/// has; a library run on one worker refuses a second thread.
///
/// Counts are plain while one host thread runs alone outside a parallel
/// section, and that is where a library spends most of its time. The
/// second thread's first call turns them atomic for good — and then waits
/// for every call still in flight, counting plainly, to leave (they are
/// counted in `HOST_ACTIVE`; `rill_host_leave` takes them off). The common
/// call, a thread the runtime knows, is one thread-local load and one
/// atomic add.
///
/// # Safety
/// Called from generated code, on any thread.
#[no_mangle]
pub unsafe extern "C" fn rill_host_enter() {
    if HOST_G.is_null() || NWORKERS < 2 {
        if !HOST_G.is_null() && NWORKERS < 2 && !on_first_thread() {
            out(b"rill: a second thread called into the library; run it with RILL_THREADS=2 or more\n");
            die();
        }
        return;
    }
    if !pthread_getspecific(TLS_KEY).is_null() {
        HOST_ACTIVE.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        return;
    }
    // a thread the runtime has not met
    let i = NWORKERS + HOST_THREADS.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
    if i >= MAX_WORKERS {
        out(b"rill: too many threads calling in at once\n");
        die();
    }
    let base = alloc_stack(EXEC_STACK_SIZE);
    WORKERS[i].index = i;
    WORKERS[i].exec_top = (base as usize + EXEC_STACK_SIZE + PAGE) & !15;
    pthread_mutex_init(&raw mut WORKERS[i].lock, core::ptr::null());
    WORKERS[i].kq.store(-1, core::sync::atomic::Ordering::Release);
    WORKERS[i].wake_fd.store(-1, core::sync::atomic::Ordering::Release);
    WORKERS[i].current = new_host_g();
    pthread_setspecific(TLS_KEY, &raw const WORKERS[i] as *const u8);
    crash_stack_here();
    // Atomic counts from here on. The flag goes up before the wait so that
    // a call entering now counts atomically, and again after it, since a
    // call that was leaving a parallel section may have put it down.
    HOST_MULTI.store(true, core::sync::atomic::Ordering::SeqCst);
    rill_mt = 1;
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    while HOST_ACTIVE.load(core::sync::atomic::Ordering::SeqCst) != 0 {
        sched_yield();
    }
    rill_mt = 1;
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    HOST_ACTIVE.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
}

/// The way out of an export; see `rill_host_enter`.
///
/// # Safety
/// Called from generated code, after `rill_host_enter` on the same thread.
#[no_mangle]
pub unsafe extern "C" fn rill_host_leave() {
    if !HOST_G.is_null() && NWORKERS >= 2 {
        HOST_ACTIVE.fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
    }
}

/// Whether this is the thread that called `rill_init`, for a library run
/// on one worker, where no thread-local tells threads apart.
unsafe fn on_first_thread() -> bool {
    pthread_self_id() == FIRST_THREAD
}

#[cfg(unix)]
unsafe fn pthread_self_id() -> usize {
    pthread_self()
}

#[cfg(not(unix))]
unsafe fn pthread_self_id() -> usize {
    0
}

static mut FIRST_THREAD: usize = 0;

/// The library's entry: the scheduler's tables and workers, then `code` —
/// the library's `main` — run on the host's thread itself.
///
/// # Safety
/// Called once from the generated `rill_init`.
#[no_mangle]
pub unsafe extern "C" fn rill_rt_host(code: extern "C" fn(*mut u8)) {
    crash_handlers_ready();
    NWORKERS = requested_threads();
    if NWORKERS > 1 && rill_parallel_build == 0 {
        let msg = b"rill: RILL_THREADS ignored - rebuild with `rill build --parallel`\n";
        write(2, msg.as_ptr(), msg.len());
        NWORKERS = 1;
    }
    pthread_mutex_init(&raw mut CHAN_LOCK, core::ptr::null());
    sync_init();
    for i in 0..NWORKERS {
        let base = alloc_stack(EXEC_STACK_SIZE);
        WORKERS[i].index = i;
        WORKERS[i].exec_top = (base as usize + EXEC_STACK_SIZE + PAGE) & !15;
        pthread_mutex_init(&raw mut WORKERS[i].lock, core::ptr::null());
        WORKERS[i].kq.store(-1, core::sync::atomic::Ordering::Release);
        WORKERS[i].wake_fd.store(-1, core::sync::atomic::Ordering::Release);
    }
    if NWORKERS > 1 {
        pthread_key_create(&raw mut TLS_KEY, core::ptr::null());
        pthread_setspecific(TLS_KEY, &raw const WORKERS[0] as *const u8);
    }
    // Counts start plain even with workers: they turn atomic for a parallel
    // section (`rill_mt_on`) and for good once a second host thread calls
    // in (`rill_host_enter`).
    rill_mt = 0;
    let g = new_host_g();
    (*g).home = 0;
    HOST_G = g;
    WORKERS[0].current = g;
    FIRST_THREAD = pthread_self_id();
    HOST_THREADS.store(1, core::sync::atomic::Ordering::Release);
    for i in 1..NWORKERS {
        let mut tid: usize = 0;
        pthread_create(
            &mut tid,
            core::ptr::null(),
            worker_entry,
            &raw mut WORKERS[i] as *mut u8,
        );
    }
    // `main` in a library is its setup: run here, on the host's stack.
    let direct: extern "C" fn() = core::mem::transmute(code);
    direct();
}

/// Whether the library is in a parallel section: counts are atomic only
/// while strands may be running on the other workers. A library spends
/// most of its time on the host's thread alone, where an atomic count is a
/// locked instruction paid for nothing, so the host says when strands are
/// about to be spawned and when they are all done.
///
/// # Safety
/// Host mode only; nothing may run on another worker while off.
#[no_mangle]
pub unsafe extern "C" fn rill_mt_on() -> i32 {
    if NWORKERS > 1 && HOST_G != core::ptr::null_mut() {
        rill_mt = 1;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    }
    1
}

/// Back to plain counts, once every strand has finished and no worker is
/// running one: waits for that, since a strand's own last releases happen
/// on its worker after its result has been received.
///
/// # Safety
/// Host mode only.
#[no_mangle]
pub unsafe extern "C" fn rill_mt_off() -> i32 {
    // Not once a second host thread has been seen: what it counts on its
    // own thread must stay atomic.
    if NWORKERS > 1
        && HOST_G != core::ptr::null_mut()
        && !HOST_MULTI.load(core::sync::atomic::Ordering::SeqCst)
    {
        while RUNNING.load(core::sync::atomic::Ordering::Acquire) != 0
            || QUEUED.load(core::sync::atomic::Ordering::Acquire) != 0
            || LIVE_STRANDS.load(core::sync::atomic::Ordering::Acquire) != 0
        {
            sched_yield();
        }
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        rill_mt = 0;
    }
    1
}

// ---- atomics on a buffer's words ---------------------------------------------
//
// For a program that keeps a protocol of its own between strands on
// several workers — a reader's mark that a writer must see before it
// starts a log over, say — where a channel would be a rendezvous too many.
// Each takes the address of an `Int` in a buffer (`ptr_offset(b, 8 * i)`)
// and is sequentially consistent, so a store on one strand and a load on
// another cannot both miss each other's writes.

/// # Safety
/// `p` must point at an aligned 64-bit word that stays allocated.
#[no_mangle]
pub unsafe extern "C" fn rill_atomic_load(p: *mut u8) -> i64 {
    core::sync::atomic::AtomicI64::from_ptr(p as *mut i64).load(core::sync::atomic::Ordering::SeqCst)
}

/// # Safety
/// As `rill_atomic_load`.
#[no_mangle]
pub unsafe extern "C" fn rill_atomic_store(p: *mut u8, v: i64) -> i64 {
    core::sync::atomic::AtomicI64::from_ptr(p as *mut i64).store(v, core::sync::atomic::Ordering::SeqCst);
    v
}

/// Adds `d` and answers the new value.
///
/// # Safety
/// As `rill_atomic_load`.
#[no_mangle]
pub unsafe extern "C" fn rill_atomic_add(p: *mut u8, d: i64) -> i64 {
    core::sync::atomic::AtomicI64::from_ptr(p as *mut i64).fetch_add(d, core::sync::atomic::Ordering::SeqCst) + d
}

/// `old` to `new` if the word holds `old`: 1 when it did.
///
/// # Safety
/// As `rill_atomic_load`.
#[no_mangle]
pub unsafe extern "C" fn rill_atomic_cas(p: *mut u8, old: i64, new: i64) -> i64 {
    core::sync::atomic::AtomicI64::from_ptr(p as *mut i64)
        .compare_exchange(old, new, core::sync::atomic::Ordering::SeqCst, core::sync::atomic::Ordering::SeqCst)
        .is_ok() as i64
}

/// The host strand's wait: strands are run on worker 0's execution stack,
/// stolen from the other workers when there are none of its own, until the
/// host is woken — by the channel operation it is parked in completing.
unsafe fn host_pump(w: *mut Worker, host: *mut G) {
    let mut idle_rounds = 0u32;
    loop {
        let flag = core::sync::atomic::AtomicI64::from_ptr(&raw mut (*host).host_woken);
        if flag.load(core::sync::atomic::Ordering::Acquire) == HOST_WAKE
            && (core::ptr::read_volatile(&(*host).selected) != -1 || (*host).on_io)
        {
            flag.store(HOST_IDLE, core::sync::atomic::Ordering::Release);
            return;
        }
        if IO_WAITERS.load(core::sync::atomic::Ordering::Acquire) > 0 {
            poll_network(w, false);
        }
        lock_runq(w);
        let mut g = (*w).runq.pop();
        unlock_runq(w);
        if g.is_null() && rill_mt != 0 && QUEUED.load(core::sync::atomic::Ordering::Acquire) > 0 {
            for i in 1..NWORKERS {
                let v = &raw mut WORKERS[i];
                lock_runq(v);
                g = (*v).runq.steal_fresh();
                unlock_runq(v);
                if !g.is_null() {
                    break;
                }
            }
        }
        if g.is_null() {
            if poll_network(w, false) > 0 {
                idle_rounds = 0;
                continue;
            }
            let quiet = RUNNING.load(core::sync::atomic::Ordering::Acquire) == 0
                && QUEUED.load(core::sync::atomic::Ordering::Acquire) == 0
                && IO_WAITERS.load(core::sync::atomic::Ordering::Acquire) == 0;
            idle_rounds = if quiet { idle_rounds + 1 } else { 0 };
            if idle_rounds > 100000 {
                out(b"rill: deadlock - the host waits and no strand can wake it\n");
                die();
            }
            sched_yield();
            continue;
        }
        bump(&QUEUED, -1);
        idle_rounds = 0;
        if !(*g).started {
            (*g).started = true;
            (*g).home = (*w).index;
            (*g).ctx.regs[12] = stack_start((*w).exec_top);
        } else if !(*g).is_main {
            restore_stack(g, (*w).exec_top);
        }
        (*w).current = g;
        bump(&RUNNING, 1);
        ctx_switch(&mut (*w).sched_ctx, &(*g).ctx);
        bump(&RUNNING, -1);
        (*w).current = host;
        if (*g).dead {
            bump(&LIVE_STRANDS, -1);
            (*g).next = (*w).free;
            (*w).free = g;
        } else if !(*g).is_main {
            save_stack(g, (*w).exec_top);
        }
    }
}

/// Set by the timer thread, read by generated code at every tail call in a
/// program that has strands in it. One byte, and never written by the code
/// that reads it, so the check is a load and a branch that is not taken.
#[no_mangle]
pub static mut rill_preempt: i8 = 0;

/// A strand asked to give up its worker.
///
/// Rill loops are tail calls, so this sits where a loop goes round: the point
/// of it is the strand that computes and never blocks, which until now held
/// its worker to the end of the program. Anything already waiting on this
/// worker could not run — not even on another worker, because a strand that
/// has started is pinned to the stack its frames live on.
///
/// Yielding is worth nothing if this strand is the only one that could run,
/// which is the common case, so the queue is asked first.
///
/// # Safety
/// Called from generated code, from strand context.
#[no_mangle]
pub unsafe extern "C" fn rill_preempt_yield() {
    // Written the way the generated code reads it. Nothing rests on who wins a
    // race for this byte: losing one costs a turn taken a slice late.
    core::ptr::write_volatile(&raw mut rill_preempt, 0);
    let w = me();
    // A strand parked on a descriptor, a timer or a sync is not in any
    // queue; it is the poll that makes it runnable, and the poll used to
    // run only when a worker had nothing to do — which a strand that never
    // blocks never lets happen. So when something is parked that way the
    // poll is made here, and a turn is given up only if it found something.
    if IO_WAITERS.load(core::sync::atomic::Ordering::Acquire) > 0 {
        poll_network(w, false);
    }
    lock_runq(w);
    let waiting = !(*w).runq.head.is_null();
    unlock_runq(w);
    if !waiting {
        return;
    }
    // Behind this worker's own queue, then back to the scheduler, which saves
    // this strand's stack exactly as it does for one parked on a channel.
    make_ready((*w).current);
    block_current();
}

/// Raises the flag every so often, for as long as the program is running.
///
/// A thread rather than a timer signal: a handler would have to be safe on
/// whichever strand stack it landed on, and this only has to store one byte.
/// It is started only when the scheduler is, so a program without strands
/// neither has this thread nor pays for the check that reads what it writes.
///
/// Taking a turn away from a strand is only worth anything if somebody else is
/// waiting for one, and a server spends most of its life with nobody waiting —
/// so when the queues are empty this waits longer between looks. It costs the
/// first strand to start hogging a worker one slow tick before it is noticed,
/// and it means an idle program is not waking a sleeping core a thousand times
/// a second to answer a question whose answer is no.
extern "C" fn preempt_timer(_arg: *mut u8) -> *mut u8 {
    unsafe {
        let mut nanos = PREEMPT_SLICE_NANOS;
        while !STOP.load(core::sync::atomic::Ordering::Acquire) {
            sleep_nanos(nanos);
            if QUEUED.load(core::sync::atomic::Ordering::Acquire) > 0
                || IO_WAITERS.load(core::sync::atomic::Ordering::Acquire) > 0
            {
                core::ptr::write_volatile(&raw mut rill_preempt, 1);
                nanos = PREEMPT_SLICE_NANOS;
            } else {
                nanos = PREEMPT_SLICE_NANOS * PREEMPT_IDLE_FACTOR;
            }
        }
    }
    core::ptr::null_mut()
}

/// # Safety
/// Called from generated code; `env` ownership transfers to the strand.
#[no_mangle]
pub unsafe extern "C" fn rill_spawn(code: extern "C" fn(*mut u8), env: *mut u8) {
    let w = me();
    lock_runq(w);
    let g = new_spawned_g(w, code, env);
    bump(&LIVE_STRANDS, 1);
    (*w).runq.push(g);
    bump(&QUEUED, 1);
    unlock_runq(w);
    if rill_mt != 0 {
        wake_a_stealer();
    }
}

/// Runs strands until the program is done. Worker 0 is the thread that
/// called `main`; the rest are started only when `RILL_THREADS` asks for them.
unsafe fn worker_loop(w: *mut Worker) {
    // Consecutive polls that found nothing anywhere; a genuine deadlock keeps
    // reporting the same thing, whereas a momentary gap between one worker
    // queueing work and another looking for it does not.
    let mut idle_rounds = 0u32;
    loop {
        // Asked before taking work, not only when there is none left. A single
        // worker already behaved this way — the one running `main` returns the
        // moment `main` dies, queue and all — so this is what makes several
        // workers abandon the same strands one would, instead of running
        // however many of them happened to be queued.
        if STOP.load(core::sync::atomic::Ordering::Acquire) {
            return;
        }
        // A strand waiting on a descriptor, a timer or a sync is woken by a
        // poll, and the poll used to run only when nothing was runnable: a
        // strand that never blocked kept the worker from ever looking, and
        // a writer parked on its commit's fsync starved behind a reader
        // that loops. So the poll is made, without waiting, before every
        // strand is taken while anything is parked that way.
        if IO_WAITERS.load(core::sync::atomic::Ordering::Acquire) > 0 {
            poll_network(w, false);
        }
        lock_runq(w);
        let mut g = (*w).runq.pop();
        unlock_runq(w);
        // Nothing of our own, and something queued somewhere: take unstarted
        // work from another worker. The count is asked first because the
        // scan takes every other worker's queue lock, and a worker with
        // nothing to do would otherwise take the lock of a worker that has —
        // over and over, on the very cache line that worker is using. Six
        // idle workers spinning that way cost the four busy ones a quarter
        // of their speed.
        if g.is_null() && rill_mt != 0 && QUEUED.load(core::sync::atomic::Ordering::Acquire) > 0 {
            for i in 0..NWORKERS {
                if i == (*w).index {
                    continue;
                }
                let v = &raw mut WORKERS[i];
                lock_runq(v);
                g = (*v).runq.steal_fresh();
                unlock_runq(v);
                if !g.is_null() {
                    break;
                }
            }
        }
        if !g.is_null() {
            bump(&QUEUED, -1);
            idle_rounds = 0;
        } else {
            if !MAIN_G.is_null() && (*MAIN_G).dead {
                return;
            }
            // Anything the network made ready becomes runnable here.
            if poll_network(w, true) > 0 {
                idle_rounds = 0;
                continue;
            }
            let quiet = RUNNING.load(core::sync::atomic::Ordering::Acquire) == 0
                && QUEUED.load(core::sync::atomic::Ordering::Acquire) == 0
                && IO_WAITERS.load(core::sync::atomic::Ordering::Acquire) == 0;
            idle_rounds = if quiet { idle_rounds + 1 } else { 0 };
            // In host mode the host's thread is not counted as running and
            // may spawn at any moment, so a quiet spell is not a deadlock.
            if idle_rounds > 1000 && HOST_G.is_null() {
                // Nothing is executing and no queue holds work, round after
                // round: nothing can ever wake the parked strands.
                out(b"rill: deadlock - all strands are blocked\n");
                die();
            }
            sched_yield();
            continue;
        }
        if !(*g).started {
            // First run: adopt this worker's execution stack.
            (*g).started = true;
            (*g).home = (*w).index;
            (*g).ctx.regs[12] = stack_start((*w).exec_top);
        } else if !(*g).is_main {
            restore_stack(g, (*w).exec_top);
        }
        (*w).current = g;
        bump(&RUNNING, 1);

        ctx_switch(&mut (*w).sched_ctx, &(*g).ctx);

        bump(&RUNNING, -1);
        (*w).current = core::ptr::null_mut();
        if (*g).dead {
            if (*g).is_main {
                STOP.store(true, core::sync::atomic::Ordering::Release);
                return;
            }
            bump(&LIVE_STRANDS, -1);
            (*g).next = (*w).free;
            (*w).free = g;
        } else if !(*g).is_main {
            // Parked on a channel. Saving here, on the strand's own worker,
            // is what lets a waker on another thread queue it immediately.
            save_stack(g, (*w).exec_top);
        }
    }
}

extern "C" fn worker_entry(arg: *mut u8) -> *mut u8 {
    unsafe {
        let w = arg as *mut Worker;
        pthread_setspecific(TLS_KEY, arg);
        crash_stack_here();
        worker_loop(w);
        // Past this point this thread reads no strand again, which is the
        // fact `sweep_abandoned` waits for. Release so that everything this
        // worker did to a strand is visible to whoever sees the count.
        WORKERS_EXITED.fetch_add(1, core::sync::atomic::Ordering::Release);
    }
    core::ptr::null_mut()
}

/// Waits for the other workers to leave their loops, so that what the strands
/// left behind can be touched without racing whoever was running one.
///
/// This is a join in everything but name, and it is not `pthread_join` for the
/// reason joining was rejected in the first place: a strand that never blocks
/// would never let go of its worker, and waiting on it forever is precisely
/// what abandoning is supposed to avoid. So the wait is bounded, and saying
/// "no" is a normal outcome rather than a failure — the strands simply stay as
/// they were, which is what happened before any of this existed.
///
/// `STOP` is already set by the time this runs: the worker that ran `main` set
/// it as `main` died. A worker asleep in the poller has not looked at it, so
/// each is woken once; a worker inside a strand will look when the strand next
/// blocks or ends.
unsafe fn quiesce_workers() -> bool {
    if NWORKERS <= 1 {
        return true;
    }
    for i in 1..NWORKERS {
        wake_worker(&raw mut WORKERS[i]);
    }
    // Enough yields that a worker only fails to arrive if it is inside a
    // strand that does not intend to stop; the ordinary case is a handful.
    let mut spins = 0u32;
    while WORKERS_EXITED.load(core::sync::atomic::Ordering::Acquire) < NWORKERS - 1 {
        if spins > 10_000 {
            return false;
        }
        spins += 1;
        sched_yield();
    }
    true
}

/// Releases what the strands abandoned at exit.
///
/// `main` returning ends the program whether or not the strands it started
/// have finished, which is the semantics we want — but a strand that never
/// reaches the end of its body never reaches the `rill_env_release` there, so
/// its environment, and everything the environment captured, is still held.
/// One strand parked on a channel it will now never be sent on is enough, and
/// a worker loop that spawned and abandoned is enough to make it unbounded.
///
/// A strand that finished has already released its own environment and is
/// marked dead, so this cannot release anything twice.
///
/// With more than one worker the others have to be known to have stopped
/// first, or this would race a worker still reading the strand it is running.
/// `quiesce_workers` is what establishes that; if it cannot, nothing here runs
/// and the strands are left as they were.
unsafe fn sweep_abandoned() {
    if !quiesce_workers() {
        return;
    }
    if LIVE_STRANDS.load(core::sync::atomic::Ordering::Acquire) == 0 {
        return;
    }
    let mut g = ALL_GS.load(core::sync::atomic::Ordering::Acquire) as *mut G;
    while !g.is_null() {
        if !(*g).dead {
            if !(*g).env.is_null() {
                rill_env_release((*g).env);
                (*g).env = core::ptr::null_mut();
            }
            // One that never ran holds nothing else. One that did has its own
            // stack, and what is on it stays: see `rill_alloc_report`.
            if (*g).started {
                ABANDONED += 1;
            }
            (*g).dead = true;
        }
        // The copied-out stacks are deliberately left alone. They are plain
        // `malloc` blocks, they are not what the live-allocation count counts,
        // and this runs with the program about to end — handing back a hundred
        // thousand of them, one per strand a program like `ring` made, cost
        // more than everything else here put together.
        g = (*g).all_next;
    }
}

/// Reads `RILL_THREADS`, clamped to the worker table.
unsafe fn requested_threads() -> usize {
    let v = getenv(b"RILL_THREADS\0".as_ptr());
    if v.is_null() {
        return 1;
    }
    let mut n = 0usize;
    let mut i = 0;
    while *v.add(i) != 0 {
        let c = *v.add(i);
        if !c.is_ascii_digit() {
            break;
        }
        n = n * 10 + (c - b'0') as usize;
        i += 1;
    }
    n.clamp(1, MAX_WORKERS)
}

/// Runs `code` as the main strand until it returns, Go-style: strands
/// still in flight at that moment are abandoned.
///
/// # Safety
/// Called once from the generated C `main`.
/// The entry for a program that never spawns, never touches a channel and
/// never waits on a socket: there is nothing for a scheduler to schedule, so
/// `main` simply runs on the process stack. Nothing here references a worker,
/// a queue or the poller, which is what lets the linker drop all of it — a
/// third of the runtime's code in a program that was never going to use it.
#[no_mangle]
/// `rill_main` takes no environment — it captures nothing — so this is typed
/// the way the compiler actually emits it. Calling a nullary function through
/// a one-argument pointer is harmless where a call is just a jump, but wasm
/// checks the signature of an indirect call against the callee's and traps on
/// a mismatch, so the two have to agree exactly.
pub unsafe extern "C" fn rill_rt_run_direct(code: extern "C" fn()) {
    NWORKERS = 1;
    rill_mt = 0;
    atexit(flush_at_exit);
    crash_handlers_ready();
    alloc_checks_wanted();
    if ALLOC_CHECK && rill_parallel_build != 0 {
        // The checks live in the runtime's allocator, and a parallel build
        // goes there only while `rill_mt` says workers share; the inline
        // fast paths are skipped from here on, at the price of atomic
        // counts, and the worker they look up is the one there is.
        pthread_key_create(&raw mut TLS_KEY, core::ptr::null());
        pthread_setspecific(TLS_KEY, &raw const WORKERS[0] as *const u8);
        rill_mt = 1;
    }
    code();
}

/// `RILL_ALLOC_CHECK`: 1 for the allocator's checks, 2 for the quarantine
/// on top. Only a `--parallel` build can take them all the way — a plain
/// build pops and pushes the pool's blocks inline, where no check is.
unsafe fn alloc_checks_wanted() {
    let check = getenv(b"RILL_ALLOC_CHECK\0".as_ptr());
    ALLOC_CHECK = !check.is_null();
    ALLOC_QUAR = ALLOC_CHECK && *check == b'2';
    if ALLOC_CHECK && rill_parallel_build == 0 {
        err(b"rill: RILL_ALLOC_CHECK covers only what the runtime allocates; build with --parallel for all of it\n");
    }
}

#[no_mangle]
pub unsafe extern "C" fn rill_rt_run(code: extern "C" fn(*mut u8)) {
    atexit(flush_at_exit);
    crash_handlers_ready();
    alloc_checks_wanted();
    NWORKERS = requested_threads();
    if ALLOC_CHECK && rill_parallel_build != 0 && NWORKERS < 2 {
        // as in `rill_rt_run_direct`: the checks need every block to pass
        // through the runtime, which takes `rill_mt` on
        NWORKERS = 2;
    }
    if NWORKERS > 1 && rill_parallel_build == 0 {
        // The generated code updates reference counts without synchronizing,
        // which is what makes single-worker programs fast. Running it on
        // several workers would be unsound, so say so and stay on one.
        let msg = b"rill: RILL_THREADS ignored - rebuild with `rill build --parallel`\n";
        write(2, msg.as_ptr(), msg.len());
        NWORKERS = 1;
    }
    pthread_mutex_init(&raw mut CHAN_LOCK, core::ptr::null());
    sync_init();
    for i in 0..NWORKERS {
        let base = alloc_stack(EXEC_STACK_SIZE);
        WORKERS[i].index = i;
        WORKERS[i].exec_top = (base as usize + EXEC_STACK_SIZE + PAGE) & !15;
        // The statics start zeroed, so the parts that are not zero at rest
        // are set here: a usable mutex, and "no poller yet". Zero would be a
        // valid descriptor — it is stdin — so the "not opened" marker has to
        // be -1 and cannot be left to the loader.
        pthread_mutex_init(&raw mut WORKERS[i].lock, core::ptr::null());
        WORKERS[i].kq.store(-1, core::sync::atomic::Ordering::Release);
        WORKERS[i].wake_fd.store(-1, core::sync::atomic::Ordering::Release);
    }
    // The key has to exist before `rill_mt` turns on, because from that moment
    // allocation looks up the calling worker through it.
    if NWORKERS > 1 {
        pthread_key_create(&raw mut TLS_KEY, core::ptr::null());
        pthread_setspecific(TLS_KEY, &raw const WORKERS[0] as *const u8);
    }
    // Must be set before any worker exists: every refcount operation from
    // here on, on every thread, then agrees on being atomic.
    rill_mt = (NWORKERS > 1) as i8;

    let main_g = new_main_g(code);
    MAIN_G = main_g;
    WORKERS[0].runq.push(main_g);
    QUEUED.store(1, core::sync::atomic::Ordering::Release);

    for i in 1..NWORKERS {
        let mut tid: usize = 0;
        pthread_create(
            &mut tid,
            core::ptr::null(),
            worker_entry,
            &raw mut WORKERS[i] as *mut u8,
        );
    }
    // Touches nothing a strand owns — one byte, written and read — so it does
    // not make this a multi-worker program and reference counts stay plain.
    let mut timer_tid: usize = 0;
    pthread_create(&mut timer_tid, core::ptr::null(), preempt_timer, core::ptr::null_mut());
    worker_loop(&raw mut WORKERS[0]);
    sweep_abandoned();
}

// ---- channels -------------------------------------------------------------
//
// A strand blocked on `select` waits on several channels at once, so the
// wait queues hold small `Waiter` nodes rather than strands directly: one
// node per case, all pointing back at the same strand. The first channel to
// reach it claims it by stamping the winning case number; nodes belonging to an
// already-claimed strand are skipped and dropped when encountered, and the
// strand unlinks whatever is left when it wakes.

/// Arms a deadline for a `select`, or -1 where there is no poller to arm one
/// in. A build without sockets has no `select` that can wait either, so the
/// arm below is never the one that runs.
#[cfg(unix)]
unsafe fn select_deadline(ms: i64) -> i32 {
    net::arm_select_deadline(ms)
}

#[cfg(not(unix))]
unsafe fn select_deadline(_ms: i64) -> i32 {
    -1
}

/// A strand's place in one channel's queue.
#[repr(C)]
struct Waiter {
    next: *mut Waiter,
    g: *mut G,
    /// The value being handed over, in either direction.
    item: [u64; 2],
    /// Which `select` case this node belongs to (0 for a plain send/recv).
    case: i64,
    chan: *mut Chan,
    /// Set when this waiter was released because the channel closed rather
    /// than because a peer arrived. Without it a woken receiver cannot tell
    /// the two apart: a value handed over just before the close would look
    /// like a close, and be lost.
    freed_by_close: i64,
}

const WAITER_INIT: Waiter = Waiter {
    next: core::ptr::null_mut(),
    g: core::ptr::null_mut(),
    item: [0; 2],
    case: 0,
    chan: core::ptr::null_mut(),
    freed_by_close: 0,
};

struct WQueue {
    head: *mut Waiter,
    tail: *mut Waiter,
}

impl WQueue {
    const fn new() -> WQueue {
        WQueue { head: core::ptr::null_mut(), tail: core::ptr::null_mut() }
    }

    unsafe fn push(&mut self, w: *mut Waiter) {
        (*w).next = core::ptr::null_mut();
        if self.tail.is_null() {
            self.head = w;
        } else {
            (*self.tail).next = w;
        }
        self.tail = w;
    }

    /// The next waiter whose strand this call has claimed — for the node's
    /// case, by a compare-and-swap on the strand's `selected` — dropping
    /// any that another channel or the poller got to first. The claim is
    /// made here, under this channel's lock alone, because a strand in a
    /// `select` sits in several channels' queues, each with a lock of its
    /// own, and two of them may reach it at once.
    unsafe fn pop_claimed(&mut self) -> *mut Waiter {
        loop {
            let w = self.head;
            if w.is_null() {
                return core::ptr::null_mut();
            }
            self.head = (*w).next;
            if self.head.is_null() {
                self.tail = core::ptr::null_mut();
            }
            (*w).next = core::ptr::null_mut();
            if try_claim(w) {
                return w;
            }
        }
    }

    /// Unlinks a node if it is still queued.
    unsafe fn remove(&mut self, w: *mut Waiter) {
        let mut prev: *mut Waiter = core::ptr::null_mut();
        let mut cur = self.head;
        while !cur.is_null() {
            if cur == w {
                if prev.is_null() {
                    self.head = (*cur).next;
                } else {
                    (*prev).next = (*cur).next;
                }
                if self.tail == cur {
                    self.tail = prev;
                }
                (*cur).next = core::ptr::null_mut();
                return;
            }
            prev = cur;
            cur = (*cur).next;
        }
    }
}

/// Claims the waiter's strand for the waiter's case: true when this call
/// got there first, false when another channel or the poller did.
unsafe fn try_claim(w: *mut Waiter) -> bool {
    let sel = core::sync::atomic::AtomicI64::from_ptr(&raw mut (*(*w).g).selected);
    sel.compare_exchange(
        -1,
        (*w).case,
        core::sync::atomic::Ordering::AcqRel,
        core::sync::atomic::Ordering::Acquire,
    )
    .is_ok()
}

/// A claimed strand made runnable, once whatever it is to find has been
/// written for it.
unsafe fn wake(w: *mut Waiter) {
    make_ready((*w).g);
}

/// Each channel has a lock of its own — a spin on a word, since what it
/// covers is a few pointer moves — so that strands on different cores
/// working different channels do not queue up on one. Nothing on a single
/// worker, where nothing runs at once.
#[inline(always)]
unsafe fn chan_lock(c: *mut Chan) {
    if rill_mt != 0 {
        while (*c)
            .lk
            .compare_exchange_weak(
                0,
                1,
                core::sync::atomic::Ordering::Acquire,
                core::sync::atomic::Ordering::Relaxed,
            )
            .is_err()
        {
            core::hint::spin_loop();
        }
    }
}

#[inline(always)]
unsafe fn chan_unlock(c: *mut Chan) {
    if rill_mt != 0 {
        (*c).lk.store(0, core::sync::atomic::Ordering::Release);
    }
}

/// A channel, reference-counted like every heap object. With capacity zero it
/// is a rendezvous point and the value in flight lives in the waiter; with a
/// capacity it also carries a ring buffer, allocated in the same block right
/// after this header.
#[repr(C)]
struct Chan {
    rc: i64,
    cap: usize,
    len: usize,
    /// Index of the oldest buffered value.
    head: usize,
    sendq: WQueue,
    recvq: WQueue,
    /// Non-zero once `close` has been called. A closed channel takes no more
    /// values; what is already buffered in it is still there to be read.
    closed: usize,
    /// The channel's own lock; see `chan_lock`.
    lk: core::sync::atomic::AtomicI32,
}

unsafe fn chan_slots(c: *mut Chan) -> *mut u64 {
    (c as *mut u8).add(core::mem::size_of::<Chan>()) as *mut u64
}

unsafe fn chan_push(c: *mut Chan, a: u64, b: u64) {
    let i = ((*c).head + (*c).len) % (*c).cap;
    let slots = chan_slots(c);
    *slots.add(i * 2) = a;
    *slots.add(i * 2 + 1) = b;
    (*c).len += 1;
}

unsafe fn chan_pop(c: *mut Chan) -> TwoWords {
    let i = (*c).head;
    let slots = chan_slots(c);
    let out = TwoWords { a: *slots.add(i * 2), b: *slots.add(i * 2 + 1) };
    (*c).head = (i + 1) % (*c).cap;
    (*c).len -= 1;
    out
}

unsafe fn chan_size(cap: usize) -> u64 {
    (core::mem::size_of::<Chan>() + cap * 16) as u64
}

/// Takes a buffered value and lets a sender that was blocked on a full buffer
/// move in behind it.
unsafe fn chan_take_buffered(c: *mut Chan) -> TwoWords {
    let out = chan_pop(c);
    let s = (*c).sendq.pop_claimed();
    if !s.is_null() {
        let item = (*s).item;
        chan_push(c, item[0], item[1]);
        wake(s);
    }
    out
}

#[repr(C)]
pub struct TwoWords {
    pub a: u64,
    pub b: u64,
}

/// # Safety
/// Returns an owned channel (rc = 1). `cap` is the buffer size; zero makes it
/// a rendezvous channel.
#[no_mangle]
pub unsafe extern "C" fn rill_chan_new(cap: i64) -> *mut u8 {
    let cap = cap.max(0) as usize;
    let c = rill_alloc(chan_size(cap)) as *mut Chan;
    core::ptr::write(
        c,
        Chan {
            rc: 1,
            cap,
            len: 0,
            head: 0,
            sendq: WQueue::new(),
            recvq: WQueue::new(),
            closed: 0,
            lk: core::sync::atomic::AtomicI32::new(0),
        },
    );
    c as *mut u8
}

/// # Safety
/// `p` must be a live channel.
#[no_mangle]
pub unsafe extern "C" fn rill_chan_release(p: *mut u8) {
    let size = chan_size((*(p as *mut Chan)).cap);
    if rc_dec(p as *mut i64) == 0 {
        rill_free_sized(p, size);
    }
}

/// Closes a channel: nothing more may be sent, and everything parked on it
/// wakes up now rather than waiting for a peer that is never coming.
///
/// This is what a program that has to *stop* needs and had no way to say. A
/// strand parked on `recv` was parked until a value arrived, so tearing down
/// the thing that was going to send one left the strand where it was, holding
/// its stack, forever. Closing is the broadcast that reaches all of them at
/// once, however many there are, which sending a value cannot be.
///
/// Closing twice is not an error. A teardown that runs from two directions
/// should not have to arrange which of them gets there first.
///
/// # Safety
/// `p` must be a live channel.
#[no_mangle]
pub unsafe extern "C" fn rill_chan_close(p: *mut u8) {
    let c = p as *mut Chan;
    chan_lock(c);
    if (*c).closed != 0 {
        chan_unlock(c);
        return;
    }
    (*c).closed = 1;
    // Everyone waiting, in both directions, and marked so that each can tell
    // what woke it. A receiver will answer `None` or say what it was asked to
    // do that it cannot; a sender has been sending into something that no
    // longer takes anything, which is a mistake and is reported as one.
    loop {
        let w = (*c).recvq.pop_claimed();
        if w.is_null() {
            break;
        }
        (*w).freed_by_close = 1;
        wake(w);
    }
    loop {
        let w = (*c).sendq.pop_claimed();
        if w.is_null() {
            break;
        }
        (*w).freed_by_close = 1;
        wake(w);
    }
    chan_unlock(c);
}

/// Whether `close` has been called on this channel.
///
/// # Safety
/// `p` must be a live channel.
#[no_mangle]
pub unsafe extern "C" fn rill_chan_closed(p: *mut u8) -> i8 {
    let c = p as *mut Chan;
    chan_lock(c);
    let r = ((*c).closed != 0) as i8;
    chan_unlock(c);
    r
}

/// What a program is told when it sends into a channel nothing will ever read.
///
/// Ending the program is the same answer an index outside a buffer gets, and
/// for the same reason: there is no value to return that would not be a lie,
/// and carrying on would lose the value silently. The rule that avoids it is
/// the one Go arrived at — whoever sends is whoever closes — and where two
/// strands really do race, `select` on a second channel is how one of them
/// finds out in time.
unsafe fn die_on_closed(what: &[u8]) -> ! {
    out(what);
    die()
}

/// Sends one value, parking until it can be handed over or buffered.
///
/// # Safety
/// `p` must be a live channel; called from strand context.
#[no_mangle]
pub unsafe extern "C" fn rill_chan_send(p: *mut u8, a: u64, b: u64) {
    let c = p as *mut Chan;
    chan_lock(c);
    if (*c).closed != 0 {
        chan_unlock(c);
        die_on_closed(b"rill: send on a closed channel\n");
    }
    // A waiting receiver takes it directly, whether or not there is a buffer.
    let r = (*c).recvq.pop_claimed();
    if !r.is_null() {
        (*r).item = [a, b];
        wake(r);
        chan_unlock(c);
        return;
    }
    if (*c).len < (*c).cap {
        chan_push(c, a, b);
        chan_unlock(c);
        return;
    }
    let g = (*me()).current;
    (*g).selected = -1;
    (*g).waiter = Waiter { next: core::ptr::null_mut(), g, item: [a, b], case: 0, chan: c, freed_by_close: 0 };
    (*c).sendq.push(&raw mut (*g).waiter);
    chan_unlock(c);
    block_current();
    if (*g).waiter.freed_by_close != 0 {
        die_on_closed(b"rill: send on a channel that closed while waiting\n");
    }
}

/// Receives one value, parking until a sender or a buffered value arrives.
///
/// # Safety
/// `p` must be a live channel; called from strand context.
#[no_mangle]
pub unsafe extern "C" fn rill_chan_recv(p: *mut u8) -> TwoWords {
    let c = p as *mut Chan;
    chan_lock(c);
    if (*c).len > 0 {
        let out = chan_take_buffered(c);
        chan_unlock(c);
        return out;
    }
    let s = (*c).sendq.pop_claimed();
    if !s.is_null() {
        let item = (*s).item;
        wake(s);
        chan_unlock(c);
        return TwoWords { a: item[0], b: item[1] };
    }
    if (*c).closed != 0 {
        chan_unlock(c);
        die_on_closed(b"rill: recv on a closed channel - use recv_opt\n");
    }
    let g = (*me()).current;
    (*g).selected = -1;
    (*g).waiter =
        Waiter { next: core::ptr::null_mut(), g, item: [0, 0], case: 0, chan: c, freed_by_close: 0 };
    (*c).recvq.push(&raw mut (*g).waiter);
    chan_unlock(c);
    block_current();
    if (*g).waiter.freed_by_close != 0 {
        die_on_closed(b"rill: recv on a channel that closed while waiting - use recv_opt\n");
    }
    let item = (*g).waiter.item;
    TwoWords { a: item[0], b: item[1] }
}

/// Waits for the next value and says whether there was one: true with the
/// value left in the strand's own slot, false once the channel is closed and
/// everything already in it has been read.
///
/// This and `chan_taken` are the two halves the prelude's `recv_opt` is
/// written from, in the way `map_get` is written from `map_find` and
/// `map_val_at`. They are split because the answer has two shapes and a
/// runtime call has one: the value cannot be returned when there is no value,
/// and there is no word that would stand for its absence without being a lie
/// about a string or a list.
///
/// A program is expected to write `recv_opt` and not these. They are here for
/// the same reason the map's slot readers are — a receive of a shape the
/// prelude did not think of is written from them.
///
/// Buffered values survive the close and are handed out first, so a producer
/// may close the moment it has nothing more to say without the consumer
/// losing what is already in flight.
///
/// # Safety
/// `p` must be a live channel; called from strand context.
#[no_mangle]
pub unsafe extern "C" fn rill_chan_wait(p: *mut u8) -> i8 {
    let c = p as *mut Chan;
    let g = (*me()).current;
    chan_lock(c);
    if (*c).len > 0 {
        let v = chan_take_buffered(c);
        chan_unlock(c);
        (*g).taken = [v.a, v.b];
        return 1;
    }
    let s = (*c).sendq.pop_claimed();
    if !s.is_null() {
        let item = (*s).item;
        wake(s);
        chan_unlock(c);
        (*g).taken = item;
        return 1;
    }
    if (*c).closed != 0 {
        chan_unlock(c);
        (*g).taken = [0, 0];
        return 0;
    }
    (*g).selected = -1;
    (*g).waiter =
        Waiter { next: core::ptr::null_mut(), g, item: [0, 0], case: 0, chan: c, freed_by_close: 0 };
    (*c).recvq.push(&raw mut (*g).waiter);
    chan_unlock(c);
    block_current();
    if (*g).waiter.freed_by_close != 0 {
        (*g).taken = [0, 0];
        return 0;
    }
    (*g).taken = (*g).waiter.item;
    1
}

/// The value the last `chan_wait` on this strand took. Reading it twice hands
/// the same value out twice, which is why it is not something to call on its
/// own; the channel is an argument so that the type of what comes back is
/// known, not because it is looked at.
///
/// # Safety
/// Called from strand context, and only after a `chan_wait` that said true.
#[no_mangle]
pub unsafe extern "C" fn rill_chan_taken(_p: *mut u8) -> TwoWords {
    let g = (*me()).current;
    TwoWords { a: (*g).taken[0], b: (*g).taken[1] }
}

// ---- select ---------------------------------------------------------------

// The three shapes an arm has, as generated code writes them into a case.
// Zero is an arm that receives and throws the value away — `recv(done) ->
// stop()` — and it has no constant here because nothing compares against it:
// a closed channel makes it ready, and there is nothing it has to produce.
/// An arm that sends.
const SEL_SEND: i64 = 1;
/// `timeout(ms) ->`. It queues on nothing, because there is nothing to queue
/// on: the poller answers it.
const SEL_TIMEOUT: i64 = 3;

/// What the poller stamps on a strand it has woken.
///
/// It has to be a number, because `selected` is how a strand is claimed and a
/// channel reads it to know whether anyone got there first. Positive, so a
/// channel sees the strand as taken; and further from any case number than a
/// `select` can have arms, so a `select` waking from its own deadline can tell
/// that is what happened rather than reading case zero.
const WOKEN_BY_POLLER: i64 = i64::MAX;
/// An arm that receives and binds — `x = recv(ch) -> f(x)`. A closed channel
/// makes this ready too, and then there is no `x` to give it, which is the
/// same dead end `recv` reaches and gets the same answer.
const SEL_RECV_BIND: i64 = 2;

/// One arm of a `select`, filled in by generated code.
#[repr(C)]
pub struct SelectCase {
    pub kind: i64,
    pub chan: *mut u8,
    pub a: u64,
    pub b: u64,
}

/// Takes every node this `select` put in a channel queue back out, so that no
/// queue is left pointing at nodes about to be freed. A timeout arm queued on
/// nothing and has nothing to take out.
///
/// # Safety
/// Called with the channel lock held; `ws` must be the `n` nodes written for
/// `cases`.
unsafe fn unqueue_select(cases: *const SelectCase, n: i64, ws: *mut Waiter) {
    for i in 0..n {
        let c = &*cases.add(i as usize);
        if c.kind == SEL_TIMEOUT {
            continue;
        }
        let w = ws.add(i as usize);
        let ch = (*w).chan;
        chan_lock(ch);
        if c.kind != SEL_SEND {
            (*ch).recvq.remove(w);
        } else {
            (*ch).sendq.remove(w);
        }
        chan_unlock(ch);
    }
}

/// The distinct channels of a `select`'s cases, in address order — the
/// order every `select` takes their locks in, so two can never wait on each
/// other. Timeout arms have none.
unsafe fn select_chans(cases: *const SelectCase, n: i64, out: &mut [*mut Chan; 64]) -> usize {
    let mut k = 0usize;
    for i in 0..n.min(64) {
        let c = &*cases.add(i as usize);
        if c.kind == SEL_TIMEOUT {
            continue;
        }
        let ch = c.chan as *mut Chan;
        let mut j = 0;
        while j < k && out[j] != ch {
            j += 1;
        }
        if j == k {
            out[k] = ch;
            k += 1;
        }
    }
    // insertion sort: a handful of arms
    for i in 1..k {
        let mut j = i;
        while j > 0 && (out[j - 1] as usize) > (out[j] as usize) {
            out.swap(j - 1, j);
            j -= 1;
        }
    }
    k
}

unsafe fn lock_chans(chans: &[*mut Chan; 64], k: usize) {
    for i in 0..k {
        chan_lock(chans[i]);
    }
}

unsafe fn unlock_chans(chans: &[*mut Chan; 64], k: usize) {
    for i in (0..k).rev() {
        chan_unlock(chans[i]);
    }
}

/// Waits until one of the cases can proceed, runs exactly that one, and
/// returns which. Cases are tried in order, so an earlier arm wins a tie. A
/// received value is written to `slot` (two words); the result is -1 when
/// nothing was ready and the `select` had a default.
///
/// # Safety
/// `cases` must point to `n` valid entries and `slot` to two words; called
/// from strand context.
#[no_mangle]
pub unsafe extern "C" fn rill_select(
    cases: *const SelectCase,
    n: i64,
    has_default: i8,
    slot: *mut u64,
) -> i64 {
    // Up to sixty-four distinct channels an arm may name; the compiler
    // never writes a `select` near that.
    let mut chans: [*mut Chan; 64] = [core::ptr::null_mut(); 64];
    let k = select_chans(cases, n, &mut chans);
    lock_chans(&chans, k);
    for i in 0..n {
        let c = &*cases.add(i as usize);
        if c.kind == SEL_TIMEOUT {
            // A deadline already past is ready now, which makes `timeout(0)`
            // a poll and costs nothing anywhere else.
            if c.a as i64 <= 0 {
                unlock_chans(&chans, k);
                return i;
            }
            continue;
        }
        let ch = c.chan as *mut Chan;
        if c.kind != SEL_SEND {
            if (*ch).len > 0 {
                let v = chan_take_buffered(ch);
                unlock_chans(&chans, k);
                *slot = v.a;
                *slot.add(1) = v.b;
                return i;
            }
            let s = (*ch).sendq.pop_claimed();
            if !s.is_null() {
                let item = (*s).item;
                wake(s);
                unlock_chans(&chans, k);
                *slot = item[0];
                *slot.add(1) = item[1];
                return i;
            }
            // A closed channel is ready, always. That is the whole of how a
            // strand waiting on several things is told to stop: close the one
            // that means stop, and every `select` watching it runs that arm.
            if (*ch).closed != 0 {
                unlock_chans(&chans, k);
                if c.kind == SEL_RECV_BIND {
                    die_on_closed(
                        b"rill: select bound a value from a closed channel - use recv_opt\n",
                    );
                }
                return i;
            }
        } else {
            if (*ch).closed != 0 {
                unlock_chans(&chans, k);
                die_on_closed(b"rill: select sends on a closed channel\n");
            }
            let r = (*ch).recvq.pop_claimed();
            if !r.is_null() {
                (*r).item = [c.a, c.b];
                wake(r);
                unlock_chans(&chans, k);
                return i;
            }
            if (*ch).len < (*ch).cap {
                chan_push(ch, c.a, c.b);
                unlock_chans(&chans, k);
                return i;
            }
        }
    }
    if has_default != 0 {
        unlock_chans(&chans, k);
        return -1;
    }

    // Nothing is ready: queue up on every channel at once. The nodes live on
    // the heap because a parked strand's stack is copied away.
    let ws = malloc(n as usize * core::mem::size_of::<Waiter>()) as *mut Waiter;
    if ws.is_null() {
        out(b"rill: out of memory in select\n");
        die();
    }
    let g = (*me()).current;
    (*g).selected = -1;
    // The shortest of the deadlines is the only one that matters: a `select`
    // answers once, and a later deadline could never be the one it answers
    // with.
    let mut soonest = -1i64;
    let mut deadline_case = -1i64;
    for i in 0..n {
        let c = &*cases.add(i as usize);
        if c.kind == SEL_TIMEOUT && (soonest < 0 || (c.a as i64) < soonest) {
            soonest = c.a as i64;
            deadline_case = i;
        }
    }
    for i in 0..n {
        let c = &*cases.add(i as usize);
        if c.kind == SEL_TIMEOUT {
            // Nothing to queue on. The poller is this arm's wait queue.
            core::ptr::write(
                ws.add(i as usize),
                WAITER_INIT,
            );
            continue;
        }
        let ch = c.chan as *mut Chan;
        let w = ws.add(i as usize);
        core::ptr::write(
            w,
            Waiter { next: core::ptr::null_mut(), g, item: [c.a, c.b], case: i, chan: ch, freed_by_close: 0 },
        );
        if c.kind != SEL_SEND {
            (*ch).recvq.push(w);
        } else {
            (*ch).sendq.push(w);
        }
    }
    // Armed while the lock is held, so that a deadline landing on another
    // worker waits at `lock()` rather than claiming a strand that has not
    // finished queueing itself.
    let mut held = 0i32;
    if deadline_case >= 0 {
        (*g).wait_ident = 0;
        IO_WAITERS.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        (*g).on_io = true;
        held = select_deadline(soonest);
        if held < 0 {
            // No deadline could be armed, and waiting on the channels alone
            // would be waiting longer than the program asked to. Answering the
            // timeout now is early rather than never.
            (*g).on_io = false;
            IO_WAITERS.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
            unlock_chans(&chans, k);
            unqueue_select(cases, n, ws);
            free(ws as *mut u8);
            return deadline_case;
        }
    }
    unlock_chans(&chans, k);
    block_current();

    let idx = (*g).selected;
    // The poller stamps its own mark, so this is the deadline and not a case.
    if idx == WOKEN_BY_POLLER {
        unqueue_select(cases, n, ws);
        if held > 0 {
            close(held);
        }
        free(ws as *mut u8);
        return deadline_case;
    }
    // A channel got there first, so the deadline is still armed on this
    // strand's behalf and this is the frame that has to take it back down.
    if deadline_case >= 0 && (*g).on_io {
        (*g).on_io = false;
        IO_WAITERS.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
    }
    let won = ws.add(idx as usize);
    let item = (*won).item;
    let by_close = (*won).freed_by_close != 0;
    let kind = (*cases.add(idx as usize)).kind;
    unqueue_select(cases, n, ws);
    if held > 0 {
        close(held);
    }
    free(ws as *mut u8);
    // The nodes are gone before anything is said about what woke this strand,
    // so a complaint here does not leave a dead node in a live channel.
    if by_close {
        if kind == SEL_SEND {
            die_on_closed(b"rill: select sends on a channel that closed while waiting\n");
        }
        if kind == SEL_RECV_BIND {
            die_on_closed(
                b"rill: select bound a value from a closed channel - use recv_opt\n",
            );
        }
    }
    *slot = item[0];
    *slot.add(1) = item[1];
    idx
}

// ---- networking -----------------------------------------------------------
//
// Sockets are always non-blocking. When an operation would block, the
// strand parks and its file descriptor is handed to kqueue; the scheduler
// polls kqueue whenever it runs out of runnable strands and wakes whoever
// became ready. So a strand blocked on the network costs a worker nothing,
// which is what makes one-strand-per-connection practical.

// The numbers, which are not the same everywhere.
//
// `AF_INET`, `SOCK_STREAM` and `F_SETFL` happen to agree across the Unixes.
// The rest do not, and none of the disagreements is a compile error: a BSD
// `SOL_SOCKET` on Linux is not a level that exists, so `setsockopt` fails
// silently and the address stays un-reusable; a BSD `O_NONBLOCK` on Linux is
// `O_APPEND`, so the socket stays blocking and the first `accept` freezes a
// worker for good. They are written out one platform at a time for that
// reason — the failure mode of getting one wrong is a hang, not a message.
const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const F_SETFL: i32 = 4;
// These two do agree, and unusually the agreement is worth writing down: every
// Unix took them from the same header and none of them renumbered. `SOL_SOCKET`
// above is the cautionary tale for the ones that did.
const IPPROTO_TCP: i32 = 6;
const TCP_NODELAY: i32 = 1;

#[cfg(not(target_os = "linux"))]
const SOL_SOCKET: i32 = 0xffff;
#[cfg(not(target_os = "linux"))]
const SO_REUSEADDR: i32 = 0x0004;
#[cfg(not(target_os = "linux"))]
const SO_ERROR: i32 = 0x1007;
#[cfg(not(target_os = "linux"))]
const O_NONBLOCK: i32 = 0x0004;

#[cfg(target_os = "linux")]
const SOL_SOCKET: i32 = 1;
#[cfg(target_os = "linux")]
const SO_REUSEADDR: i32 = 2;
#[cfg(target_os = "linux")]
const SO_ERROR: i32 = 4;
#[cfg(target_os = "linux")]
const O_NONBLOCK: i32 = 0o4000;

// A socket whose far end has gone is an error to write to. It is not a reason
// to kill the process — but by default it is exactly that, because the kernel
// raises SIGPIPE and nothing here handles it.
//
// Rust's own runtime ignores SIGPIPE before `main` runs. A Rill binary's main
// is generated code and never goes through it, so a server that wrote one line
// to a client which had just walked away died where it stood: no panic, no
// message, no line in the log, exit 141. It is the kind of fault that looks
// like a mystery for a week.
//
// Turned off per socket rather than for the whole process, because a program
// piping into `head` should still stop when `head` does and that is a pipe.
// BSD says it once with a socket option; Linux has no such option and says it
// on every send instead.
#[cfg(not(target_os = "linux"))]
const SO_NOSIGPIPE: i32 = 0x1022;
#[cfg(target_os = "linux")]
const MSG_NOSIGNAL: i32 = 0x4000;

// ---- sockets ---------------------------------------------------------------
//
// BSD sockets, which is to say: not Windows. Winsock has the same shape and
// different names, and its waiting is done with completion ports rather than
// by asking which handles are ready — so the port of this section is a rewrite
// rather than a rename, and it is not written yet. A program that opens no
// socket links and runs without it.
//
// Everything above the poller is shared; the poller itself is kqueue on the
// BSDs and epoll on Linux, behind four functions.
#[cfg(unix)]
use net::{poll_network, wake_a_stealer, wake_worker, IO_WAITERS, STRAY_WAKES};

/// Without sockets there is nothing to wait on and nobody to wake: the
/// scheduler asks after the network every time round, and on Windows the
/// answer is always the same one.
#[cfg(windows)]
unsafe fn wake_worker(_w: *mut Worker) {}
#[cfg(windows)]
unsafe fn wake_a_stealer() {}
#[cfg(windows)]
unsafe fn poll_network(_w: *mut Worker, _block: bool) -> i32 {
    0
}
#[cfg(windows)]
static IO_WAITERS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(windows)]
static STRAY_WAKES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// The same silence on wasm, for a different reason: wasip1 has no sockets to
/// offer and no worker to wake if it had.
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn wake_worker(_w: *mut Worker) {}
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn wake_a_stealer() {}
#[cfg(any(target_family = "wasm", target_os = "none"))]
unsafe fn poll_network(_w: *mut Worker, _block: bool) -> i32 {
    0
}
#[cfg(any(target_family = "wasm", target_os = "none"))]
static IO_WAITERS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
#[cfg(any(target_family = "wasm", target_os = "none"))]
static STRAY_WAKES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(unix)]
mod net {
    use super::*;

    #[cfg(not(target_os = "linux"))]
    const EAGAIN: i32 = 35;
    #[cfg(not(target_os = "linux"))]
    const EINPROGRESS: i32 = 36;
    #[cfg(target_os = "linux")]
    const EAGAIN: i32 = 11;
    #[cfg(target_os = "linux")]
    const EINPROGRESS: i32 = 115;
    const EINTR: i32 = 4;

    const READ_CHUNK: usize = 64 * 1024;

    /// How many ready strands one look at the poller may collect.
    const READY_BATCH: usize = 32;

    extern "C" {
        fn socket(domain: i32, ty: i32, protocol: i32) -> i32;
        fn bind(fd: i32, addr: *const SockAddrIn, len: u32) -> i32;
        fn listen(fd: i32, backlog: i32) -> i32;
        fn accept(fd: i32, addr: *mut u8, len: *mut u32) -> i32;
        fn recv(fd: i32, buf: *mut u8, len: usize, flags: i32) -> isize;
        fn send(fd: i32, buf: *const u8, len: usize, flags: i32) -> isize;
        fn getpeername(fd: i32, addr: *mut SockAddrIn, len: *mut u32) -> i32;
        fn getsockname(fd: i32, addr: *mut SockAddrIn, len: *mut u32) -> i32;
        fn connect(fd: i32, addr: *const SockAddrIn, len: u32) -> i32;
        fn shutdown(fd: i32, how: i32) -> i32;
        fn setsockopt(fd: i32, level: i32, name: i32, val: *const u8, len: u32) -> i32;
        fn getsockopt(fd: i32, level: i32, name: i32, val: *mut u8, len: *mut u32) -> i32;
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        /// macOS's errno: a function, because errno is per-thread and a plain
        /// global could not be.
        #[cfg(target_vendor = "apple")]
        fn __error() -> *mut i32;
        /// The same thing everywhere else it is called something else.
        #[cfg(all(unix, not(target_vendor = "apple")))]
        #[link_name = "__errno_location"]
        fn __error() -> *mut i32;
        #[cfg(windows)]
        #[link_name = "_errno"]
        fn __error() -> *mut i32;
    }

    /// The BSD one carries its own length in the first byte; Linux uses those
    /// two bytes for the family instead. Same sixteen bytes either way, which
    /// is why passing the wrong one is a silent misparse rather than an error.
    #[cfg(not(target_os = "linux"))]
    #[repr(C)]
    struct SockAddrIn {
        len: u8,
        family: u8,
        port: u16,
        addr: [u8; 4],
        zero: [u8; 8],
    }

    #[cfg(target_os = "linux")]
    #[repr(C)]
    struct SockAddrIn {
        family: u16,
        port: u16,
        addr: [u8; 4],
        zero: [u8; 8],
    }

    #[cfg(not(target_os = "linux"))]
    unsafe fn sock_addr(ip: [u8; 4], port: i64) -> SockAddrIn {
        SockAddrIn { len: 16, family: AF_INET as u8, port: (port as u16).to_be(), addr: ip, zero: [0; 8] }
    }

    #[cfg(target_os = "linux")]
    unsafe fn sock_addr(ip: [u8; 4], port: i64) -> SockAddrIn {
        SockAddrIn { family: AF_INET as u16, port: (port as u16).to_be(), addr: ip, zero: [0; 8] }
    }

    /// Strands currently parked on a file descriptor. While this is non-zero
    /// an idle scheduler waits in kqueue instead of declaring a deadlock.
    pub(super) static IO_WAITERS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

    /// Events delivered for a strand that is no longer waiting on them. Kept
    /// as a number because it is a real thing that happens and not an error:
    /// what would be an error is acting on one.
    pub(super) static STRAY_WAKES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

    unsafe fn errno() -> i32 {
        *__error()
    }

    // ---- the poller ---------------------------------------------------------
    //
    // Four operations, two implementations. Everything else in this module is
    // written once against these, because the difference between kqueue and
    // epoll is genuinely small at this scale: register interest in one
    // descriptor for one event, wait for whatever became ready, and have some
    // way to interrupt that wait from another thread.
    //
    //   poller_open   create it, and whatever it needs to be woken through
    //   poller_wake   interrupt a worker asleep in `poller_wait`
    //   poller_arm    park this strand on this descriptor, once
    //   poller_timer  park this strand until a deadline, once
    //   poller_wait   collect the strands that became runnable
    //
    // The one real asymmetry is the wakeup. kqueue has a filter for events the
    // program raises itself, so a worker can be poked through the same handle
    // it is waiting on. epoll has nothing like it and needs a real descriptor
    // in the set — an eventfd, whose entire purpose is to be that descriptor.
    //
    // The timer is the second asymmetry, and the same shape: kqueue has a
    // filter that counts time, and epoll has only descriptors, so a Linux
    // timer is a descriptor that becomes readable — which is what a timerfd
    // is. That difference is why `poller_timer` hands back something to close:
    // on Linux there is an fd behind the wait and on the BSDs there is not.

    #[cfg(not(target_os = "linux"))]
    use bsd::{
        poller_arm, poller_arm_deadline, poller_disarm, poller_forget, poller_open, poller_timer,
        poller_wait, poller_wake,
    };
    #[cfg(target_os = "linux")]
    use linux::{
        poller_arm, poller_arm_deadline, poller_disarm, poller_forget, poller_open, poller_timer,
        poller_wait, poller_wake,
    };

    #[cfg(not(target_os = "linux"))]
    mod bsd {
        use super::*;

        const EVFILT_READ: i16 = -1;
        const EVFILT_WRITE: i16 = -2;
        /// A filter that counts time down and reports once. Its `data` is
        /// milliseconds unless an `fflags` says otherwise, which is the unit
        /// `sleep_ms` wants anyway.
        const EVFILT_TIMER: i16 = -7;
        /// A filter the program triggers itself, used to wake a worker asleep
        /// in `kevent` the moment another worker queues work for it.
        const EVFILT_USER: i16 = -10;
        const EV_ADD: u16 = 0x0001;
        const EV_DELETE: u16 = 0x0002;
        const EV_ONESHOT: u16 = 0x0010;
        const EV_CLEAR: u16 = 0x0020;
        const NOTE_TRIGGER: u32 = 0x0100_0000;
        /// Identifier of that wakeup event. Filters have separate ident spaces,
        /// so this cannot collide with a file descriptor.
        const WAKE_IDENT: usize = 0;

        extern "C" {
            fn kqueue() -> i32;
            fn kevent(
                kq: i32,
                changes: *const KEvent,
                nchanges: i32,
                events: *mut KEvent,
                nevents: i32,
                timeout: *const TimeSpec,
            ) -> i32;
        }

        #[repr(C)]
        pub(super) struct KEvent {
            ident: usize,
            filter: i16,
            flags: u16,
            fflags: u32,
            data: isize,
            udata: *mut u8,
        }

        #[repr(C)]
        struct TimeSpec {
            sec: i64,
            nsec: i64,
        }

        pub(super) unsafe fn poller_open(w: *mut Worker) -> i32 {
            let kq = kqueue();
            if kq < 0 {
                out(b"rill: cannot create kqueue\n");
                die();
            }
            let ev = KEvent {
                ident: WAKE_IDENT,
                filter: EVFILT_USER,
                flags: EV_ADD | EV_CLEAR,
                fflags: 0,
                data: 0,
                udata: core::ptr::null_mut(),
            };
            kevent(kq, &ev, 1, core::ptr::null_mut(), 0, core::ptr::null());
            (*w).kq.store(kq, core::sync::atomic::Ordering::Release);
            kq
        }

        pub(super) unsafe fn poller_wake(w: *mut Worker) {
            let kq = (*w).kq.load(core::sync::atomic::Ordering::Acquire);
            if kq < 0 {
                return;
            }
            let ev = KEvent {
                ident: WAKE_IDENT,
                filter: EVFILT_USER,
                flags: 0,
                fflags: NOTE_TRIGGER,
                data: 0,
                udata: core::ptr::null_mut(),
            };
            kevent(kq, &ev, 1, core::ptr::null_mut(), 0, core::ptr::null());
        }

        /// One-shot, so nothing has to be unregistered afterwards.
        // A descriptor's registration used to be made for one wait and
        // consumed by it (`EV_ONESHOT`): a `kevent` call to arm before every
        // `kevent` call to wait, which on an echo round trip was a third
        // system call per side. On one worker a descriptor is registered
        // once, edge-triggered (`EV_CLEAR`), for the strand that waits on it,
        // and re-registered only when a different strand comes to wait — the
        // table below remembers who. That is sound because every wait is made
        // only after the operation itself said `EAGAIN`, so the next edge is
        // the one wanted, and because `poller_wait` checks the event against
        // what the strand is parked on now and drops one for a descriptor it
        // has left. `rill_sock_close` forgets the descriptor, since the kernel
        // drops its knotes with it and the number will come round again.
        // Several workers keep the one-shot path: the table is one worker's.
        struct Armed {
            read: *mut G,
            write: *mut G,
        }
        static mut ARMED: *mut Armed = core::ptr::null_mut();
        static mut ARMED_LEN: usize = 0;

        unsafe fn armed_slot(fd: i32) -> *mut Armed {
            let fd = fd as usize;
            if fd >= ARMED_LEN {
                let mut want = if ARMED_LEN == 0 { 64 } else { ARMED_LEN };
                while want <= fd {
                    want *= 2;
                }
                let fresh = calloc(want, core::mem::size_of::<Armed>()) as *mut Armed;
                if fresh.is_null() {
                    out(b"rill: out of memory in the poller\n");
                    die();
                }
                if !ARMED.is_null() {
                    core::ptr::copy_nonoverlapping(ARMED, fresh, ARMED_LEN);
                    free(ARMED as *mut u8);
                }
                ARMED = fresh;
                ARMED_LEN = want;
            }
            ARMED.add(fd)
        }

        pub(super) unsafe fn poller_forget(fd: i32) {
            if rill_mt == 0 && fd >= 0 && (fd as usize) < ARMED_LEN {
                let s = ARMED.add(fd as usize);
                (*s).read = core::ptr::null_mut();
                (*s).write = core::ptr::null_mut();
            }
        }

        /// Registers `fd` for `g` if it is not already; `true` when the
        /// poller will deliver its next edge to `g`.
        unsafe fn arm_persistent(kq: i32, fd: i32, writable: bool, g: *mut G) -> bool {
            let s = armed_slot(fd);
            let owner = if writable { &mut (*s).write } else { &mut (*s).read };
            if *owner == g {
                return true;
            }
            let ev = KEvent {
                ident: fd as usize,
                filter: if writable { EVFILT_WRITE } else { EVFILT_READ },
                flags: EV_ADD | EV_CLEAR,
                fflags: 0,
                data: 0,
                udata: g as *mut u8,
            };
            if kevent(kq, &ev, 1, core::ptr::null_mut(), 0, core::ptr::null()) < 0 {
                return false;
            }
            *owner = g;
            true
        }

        pub(super) unsafe fn poller_arm(kq: i32, fd: i32, writable: bool, g: *mut G) -> bool {
            if rill_mt == 0 {
                return arm_persistent(kq, fd, writable, g);
            }
            let ev = KEvent {
                ident: fd as usize,
                filter: if writable { EVFILT_WRITE } else { EVFILT_READ },
                flags: EV_ADD | EV_ONESHOT,
                fflags: 0,
                data: 0,
                udata: g as *mut u8,
            };
            kevent(kq, &ev, 1, core::ptr::null_mut(), 0, core::ptr::null()) >= 0
        }

        /// One-shot too, and identified by the strand itself: filters have
        /// separate ident spaces, so a timer keyed on a `G` cannot collide
        /// with a socket keyed on a descriptor. Nothing is left behind to
        /// close, hence the zero.
        pub(super) unsafe fn poller_timer(kq: i32, ms: i64, g: *mut G) -> i32 {
            let ev = KEvent {
                ident: g as usize,
                filter: EVFILT_TIMER,
                flags: EV_ADD | EV_ONESHOT,
                fflags: 0,
                data: ms as isize,
                udata: g as *mut u8,
            };
            if kevent(kq, &ev, 1, core::ptr::null_mut(), 0, core::ptr::null()) < 0 {
                return -1;
            }
            0
        }

        /// The descriptor and a deadline at once, so whichever comes first
        /// wakes the strand. Both are one-shot; the one that did not fire is
        /// deleted on the way out, and deleting what is no longer there is a
        /// harmless `ENOENT`.
        pub(super) unsafe fn poller_arm_deadline(kq: i32, fd: i32, writable: bool, ms: i64, g: *mut G) -> i32 {
            if rill_mt == 0 {
                if !arm_persistent(kq, fd, writable, g) {
                    return -1;
                }
                return poller_timer(kq, ms, g);
            }
            let evs = [
                KEvent {
                    ident: fd as usize,
                    filter: if writable { EVFILT_WRITE } else { EVFILT_READ },
                    flags: EV_ADD | EV_ONESHOT,
                    fflags: 0,
                    data: 0,
                    udata: g as *mut u8,
                },
                KEvent {
                    ident: g as usize,
                    filter: EVFILT_TIMER,
                    flags: EV_ADD | EV_ONESHOT,
                    fflags: 0,
                    data: ms as isize,
                    udata: g as *mut u8,
                },
            ];
            if kevent(kq, evs.as_ptr(), 2, core::ptr::null_mut(), 0, core::ptr::null()) < 0 {
                return -1;
            }
            0
        }

        /// Takes both registrations back down.
        pub(super) unsafe fn poller_disarm(kq: i32, fd: i32, writable: bool, _held: i32, g: *mut G) {
            if rill_mt == 0 {
                // The descriptor stays registered; only the deadline goes.
                let ev = KEvent {
                    ident: g as usize,
                    filter: EVFILT_TIMER,
                    flags: EV_DELETE,
                    fflags: 0,
                    data: 0,
                    udata: core::ptr::null_mut(),
                };
                kevent(kq, &ev, 1, core::ptr::null_mut(), 0, core::ptr::null());
                return;
            }
            let evs = [
                KEvent {
                    ident: fd as usize,
                    filter: if writable { EVFILT_WRITE } else { EVFILT_READ },
                    flags: EV_DELETE,
                    fflags: 0,
                    data: 0,
                    udata: core::ptr::null_mut(),
                },
                KEvent {
                    ident: g as usize,
                    filter: EVFILT_TIMER,
                    flags: EV_DELETE,
                    fflags: 0,
                    data: 0,
                    udata: core::ptr::null_mut(),
                },
            ];
            kevent(kq, evs.as_ptr(), 2, core::ptr::null_mut(), 0, core::ptr::null());
        }

        pub(super) unsafe fn poller_wait(kq: i32, ms: i32, ready: *mut *mut G) -> usize {
            let mut events: [KEvent; READY_BATCH] = core::mem::zeroed();
            let ts = TimeSpec { sec: 0, nsec: ms as i64 * 1_000_000 };
            let n = kevent(kq, core::ptr::null(), 0, events.as_mut_ptr(), READY_BATCH as i32, &ts);
            if n <= 0 {
                return 0;
            }
            let mut got = 0usize;
            for i in 0..n as usize {
                let g = events[i].udata as *mut G;
                if g.is_null() {
                    continue;
                }
                // A registration outliving its wait may report a descriptor
                // the strand has since left; only the one it is parked on
                // now is news.
                let f = events[i].filter;
                if (f == EVFILT_READ || f == EVFILT_WRITE)
                    && rill_mt == 0
                    && (*g).wait_ident != events[i].ident + 1
                {
                    continue;
                }
                *ready.add(got) = g;
                got += 1;
            }
            got
        }
    }

    #[cfg(target_os = "linux")]
    mod linux {
        use super::*;

        const EPOLLIN: u32 = 0x001;
        const EPOLLOUT: u32 = 0x004;
        const EPOLLONESHOT: u32 = 1 << 30;
        const EPOLL_CTL_ADD: i32 = 1;
        const EPOLL_CTL_MOD: i32 = 3;
        const EEXIST: i32 = 17;
        /// The eventfd is only ever read to drain it, and must never be the
        /// thing that blocks a worker.
        const EFD_NONBLOCK: i32 = 0o4000;
        /// Same two bits as the eventfd's, under the timerfd's own names. A
        /// timer that is read must not block, and a timer must not survive an
        /// `exec` into a program that knows nothing about it.
        const TFD_NONBLOCK: i32 = 0o4000;
        const TFD_CLOEXEC: i32 = 0o2000000;

        extern "C" {
            fn epoll_create1(flags: i32) -> i32;
            fn epoll_ctl(epfd: i32, op: i32, fd: i32, ev: *mut EpollEvent) -> i32;
            fn epoll_wait(epfd: i32, events: *mut EpollEvent, maxevents: i32, timeout: i32) -> i32;
            fn eventfd(initval: u32, flags: i32) -> i32;
            fn timerfd_create(clock: i32, flags: i32) -> i32;
            fn timerfd_settime(fd: i32, flags: i32, new: *const ITimerSpec, old: *mut ITimerSpec) -> i32;
        }

        /// An interval and a first expiry. Leaving the interval at zero is what
        /// makes it fire once — this is a deadline, not a metronome, because
        /// the strand that is woken decides for itself whether to ask again.
        #[repr(C)]
        struct ITimerSpec {
            interval: Timespec,
            value: Timespec,
        }

        /// Packed on x86-64 and not on anything else — the kernel's own
        /// definition carries `__attribute__((packed))` only for the one
        /// architecture, to keep the 32-bit and 64-bit layouts identical. Get
        /// this wrong and every `data` field comes back shifted by four bytes,
        /// which reads as a wild pointer to a strand.
        #[cfg(target_arch = "x86_64")]
        #[repr(C, packed)]
        pub(super) struct EpollEvent {
            events: u32,
            data: u64,
        }

        #[cfg(not(target_arch = "x86_64"))]
        #[repr(C)]
        pub(super) struct EpollEvent {
            events: u32,
            data: u64,
        }

        pub(super) unsafe fn poller_open(w: *mut Worker) -> i32 {
            let ep = epoll_create1(0);
            if ep < 0 {
                out(b"rill: cannot create epoll\n");
                die();
            }
            // The wakeup channel. Registered level-triggered and never
            // one-shot, so it stays armed for the life of the worker.
            let efd = eventfd(0, EFD_NONBLOCK);
            if efd < 0 {
                out(b"rill: cannot create eventfd\n");
                die();
            }
            let mut ev = EpollEvent { events: EPOLLIN, data: 0 };
            epoll_ctl(ep, EPOLL_CTL_ADD, efd, &mut ev);
            (*w).wake_fd.store(efd, core::sync::atomic::Ordering::Release);
            (*w).kq.store(ep, core::sync::atomic::Ordering::Release);
            ep
        }

        pub(super) unsafe fn poller_wake(w: *mut Worker) {
            let efd = (*w).wake_fd.load(core::sync::atomic::Ordering::Acquire);
            if efd < 0 {
                return;
            }
            let one: u64 = 1;
            write(efd, &one as *const u64 as *const u8, 8);
        }

        /// `EPOLLONESHOT` disarms the descriptor after one event but leaves it
        /// in the set, so a second arm on the same descriptor is a modify and
        /// not an add. Trying the add first and falling back keeps the common
        /// case — a connection's first wait — to one syscall.
        /// Registrations here are still one per wait, so there is nothing to
        /// forget when a descriptor closes.
        pub(super) unsafe fn poller_forget(_fd: i32) {}

        pub(super) unsafe fn poller_arm(ep: i32, fd: i32, writable: bool, g: *mut G) -> bool {
            let mut ev = EpollEvent {
                events: (if writable { EPOLLOUT } else { EPOLLIN }) | EPOLLONESHOT,
                data: g as u64,
            };
            if epoll_ctl(ep, EPOLL_CTL_ADD, fd, &mut ev) >= 0 {
                return true;
            }
            if errno() != EEXIST {
                return false;
            }
            epoll_ctl(ep, EPOLL_CTL_MOD, fd, &mut ev) >= 0
        }

        /// A timer here is a descriptor that becomes readable, so it goes into
        /// the set exactly as a socket does. The descriptor is handed back for
        /// the sleeper to close on the way out: it belongs to one wait, and
        /// leaving it open would leak a file for every sleep a program takes.
        pub(super) unsafe fn poller_timer(ep: i32, ms: i64, g: *mut G) -> i32 {
            let fd = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK | TFD_CLOEXEC);
            if fd < 0 {
                return -1;
            }
            let spec = ITimerSpec {
                interval: Timespec { secs: 0, nanos: 0 },
                value: Timespec { secs: ms / 1000, nanos: (ms % 1000) * 1_000_000 },
            };
            if timerfd_settime(fd, 0, &spec, core::ptr::null_mut()) < 0 || !poller_arm(ep, fd, false, g) {
                close(fd);
                return -1;
            }
            fd
        }

        /// The descriptor and a timerfd in the same set, both pointing at the
        /// same strand. The timerfd is handed back so the waker can close it —
        /// closing is also what takes it out of the set.
        pub(super) unsafe fn poller_arm_deadline(ep: i32, fd: i32, writable: bool, ms: i64, g: *mut G) -> i32 {
            let tfd = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK | TFD_CLOEXEC);
            if tfd < 0 {
                return -1;
            }
            let spec = ITimerSpec {
                interval: Timespec { secs: 0, nanos: 0 },
                value: Timespec { secs: ms / 1000, nanos: (ms % 1000) * 1_000_000 },
            };
            if timerfd_settime(tfd, 0, &spec, core::ptr::null_mut()) < 0
                || !poller_arm(ep, tfd, false, g)
                || !poller_arm(ep, fd, writable, g)
            {
                close(tfd);
                return -1;
            }
            tfd
        }

        /// The socket's registration is one-shot and disarms itself; the timer
        /// leaves the set when its descriptor is closed.
        pub(super) unsafe fn poller_disarm(_ep: i32, _fd: i32, _writable: bool, held: i32, _g: *mut G) {
            if held > 0 {
                close(held);
            }
        }

        pub(super) unsafe fn poller_wait(ep: i32, ms: i32, ready: *mut *mut G) -> usize {
            let mut events: [EpollEvent; READY_BATCH] = core::mem::zeroed();
            let n = epoll_wait(ep, events.as_mut_ptr(), READY_BATCH as i32, ms);
            if n <= 0 {
                return 0;
            }
            let mut got = 0usize;
            for i in 0..n as usize {
                // A copy, because the field is unaligned in the packed form and
                // may not be borrowed.
                let d = core::ptr::addr_of!(events[i].data).read_unaligned();
                if d == 0 {
                    // The wakeup channel. Drain it so a level-triggered
                    // registration does not report it for ever.
                    let efd = wake_fd_of_current();
                    if efd >= 0 {
                        let mut sink: u64 = 0;
                        read(efd, &mut sink as *mut u64 as *mut u8, 8);
                    }
                    continue;
                }
                *ready.add(got) = d as *mut G;
                got += 1;
            }
            got
        }

        /// The worker running this call is the one whose poller it is.
        unsafe fn wake_fd_of_current() -> i32 {
            let w = me();
            (*w).wake_fd.load(core::sync::atomic::Ordering::Acquire)
        }
    }

    /// Opens this worker's poller on first use. Only the owning worker calls it,
    /// so the lazy creation needs no lock.
    unsafe fn ensure_poller(w: *mut Worker) -> i32 {
        let existing = (*w).kq.load(core::sync::atomic::Ordering::Acquire);
        if existing >= 0 {
            return existing;
        }
        poller_open(w)
    }

    /// Cuts short the sleep of one specific worker. Without this, work queued by
    /// another worker would sit until that sleep timed out.
    pub(super) unsafe fn wake_worker(w: *mut Worker) {
        if !(*w).asleep.load(core::sync::atomic::Ordering::Acquire) {
            return;
        }
        poller_wake(w);
    }

    /// Wakes one sleeping worker so it can come and steal. Fresh strands sit on
    /// the queue of whoever spawned them, and a worker asleep on the network would
    /// otherwise not find out there is anything to take.
    pub(super) unsafe fn wake_a_stealer() {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        for i in 0..NWORKERS {
            let v = &raw mut WORKERS[i];
            if (*v).asleep.load(core::sync::atomic::Ordering::Acquire) {
                wake_worker(v);
                return;
            }
        }
    }

    const MSG_PEEK: i32 = 0x2;

    /// Says, on the platforms that let a socket say it, that writing to a dead
    /// peer is an error rather than a signal. See `SO_NOSIGPIPE` above.
    unsafe fn no_sigpipe(fd: i32) {
        #[cfg(not(target_os = "linux"))]
        {
            let one: i32 = 1;
            setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one as *const i32 as *const u8, 4);
        }
        #[cfg(target_os = "linux")]
        let _ = fd;
    }

    /// A write that cannot raise SIGPIPE on either platform.
    unsafe fn sock_send(fd: i32, buf: *const u8, len: usize) -> isize {
        #[cfg(target_os = "linux")]
        {
            send(fd, buf, len, MSG_NOSIGNAL)
        }
        #[cfg(not(target_os = "linux"))]
        {
            write(fd, buf, len)
        }
    }

    unsafe fn set_nonblocking(fd: i32) {
        fcntl(fd, F_SETFL, O_NONBLOCK);
    }

    /// Parks the current strand until `fd` is readable or writable. The
    /// registration is one-shot, so nothing has to be unregistered afterwards.
    unsafe fn park_on_fd(fd: i32, writable: bool) {
        rill_flush_out();
        let w = me();
        let g = (*w).current;
        (*g).selected = -1;
        (*g).wait_ident = fd as usize + 1;
        let p = ensure_poller(w);
        IO_WAITERS.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        (*g).on_io = true;
        if !poller_arm(p, fd, writable, g) {
            (*g).on_io = false;
            IO_WAITERS.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
            return; // fall back to spinning on the syscall
        }
        block_current();
    }

    /// Arms a deadline for the current strand without parking it, for a
    /// `select` that has a `timeout` arm and has to queue on its channels as
    /// well. What comes back is what `poller_timer` held, to be closed after.
    pub(super) unsafe fn arm_select_deadline(ms: i64) -> i32 {
        let w = me();
        let p = ensure_poller(w);
        poller_timer(p, ms, (*w).current)
    }

    /// Parks the current strand until a pipe has a byte in it.
    ///
    /// The whole of what the file section needs from the poller, and named for
    /// what it is used for rather than exposing `park_on_fd`: a pipe is the
    /// only descriptor outside this module that anything parks on, and it
    /// parks on it for exactly one byte.
    pub(super) unsafe fn park_on_pipe(fd: i32) {
        park_on_fd(fd, false);
    }

    /// Drops what the poller remembers about a descriptor about to be closed.
    pub(super) unsafe fn forget_pipe(fd: i32) {
        poller_forget(fd);
    }

    /// Parks the current strand for `ms` milliseconds.
    ///
    /// This is the same manoeuvre as parking on a socket, and deliberately so:
    /// the strand goes into the poller and its worker goes off to run
    /// something else, so a program can hold a tick rate without a thread
    /// standing still to do it. It also means a sleeping strand counts as an
    /// I/O waiter, which is what keeps a program whose every strand is asleep
    /// from being reported as a deadlock — it is waiting on the clock, and the
    /// clock always answers.
    ///
    /// If the timer cannot be armed at all the worker sleeps instead, which is
    /// worse but is still the duration the caller asked for.
    unsafe fn park_on_timer(ms: i64) {
        let w = me();
        let g = (*w).current;
        (*g).selected = -1;
        (*g).wait_ident = 0;
        let p = ensure_poller(w);
        IO_WAITERS.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        (*g).on_io = true;
        let held = poller_timer(p, ms, g);
        if held < 0 {
            (*g).on_io = false;
            IO_WAITERS.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
            sleep_nanos(ms * 1_000_000);
            return;
        }
        block_current();
        // Back on a worker, and the descriptor the wait was made of — if this
        // platform needed one — has done its one job.
        if held > 0 {
            close(held);
        }
    }

    /// Who is at the other end, as the four bytes of an IPv4 address packed
    /// into one number — 127.0.0.1 is 2130706433 — or -1 if it cannot be had.
    ///
    /// A number rather than a string because what a server does with it is
    /// count: how many connections this address already has. Dotted quad is
    /// what a person reads, and `int_to_str` of this is not that, which is the
    /// small price.
    ///
    /// # Safety
    /// `fd` must be an open socket.
    #[no_mangle]
    pub unsafe extern "C" fn rill_sock_peer(fd: i64) -> i64 {
        let mut addr: SockAddrIn = core::mem::zeroed();
        let mut len: u32 = core::mem::size_of::<SockAddrIn>() as u32;
        if getpeername(fd as i32, &mut addr, &mut len) < 0 {
            return -1;
        }
        let a = addr.addr;
        ((a[0] as i64) << 24) | ((a[1] as i64) << 16) | ((a[2] as i64) << 8) | (a[3] as i64)
    }

    /// The port this end of the socket is on — which is the question a
    /// program asks after listening on port 0, where the kernel picks a free
    /// one and only it knows which. -1 if the socket has none.
    ///
    /// # Safety
    /// `fd` must be an open socket.
    #[no_mangle]
    pub unsafe extern "C" fn rill_sock_port(fd: i64) -> i64 {
        let mut addr: SockAddrIn = core::mem::zeroed();
        let mut len: u32 = core::mem::size_of::<SockAddrIn>() as u32;
        if getsockname(fd as i32, &mut addr, &mut len) < 0 {
            return -1;
        }
        // The port sits in the structure in network order.
        ((addr.port >> 8) & 0xff) as i64 | (((addr.port & 0xff) as i64) << 8)
    }

    /// Turns Nagle's algorithm off on `fd`, so a small write goes out at once
    /// instead of waiting for the last one to be acknowledged. Returns 1 if
    /// the kernel took it and 0 if it did not.
    ///
    /// Nagle exists to stop a program that writes a byte at a time from
    /// filling the network with headers, and it does that by holding a small
    /// segment until the previous one is acknowledged. That is the right trade
    /// for a file being copied and the wrong one for a server that sends a
    /// small message on a clock: the moment the clock is faster than the round
    /// trip, every message waits on the one before it, and the wait is the
    /// difference between the two — which is a delay nobody can see in the
    /// code and nothing in the program can measure.
    ///
    /// It is a call rather than a default because it is a real trade and the
    /// caller is the one who knows: a program that writes in small pieces and
    /// does not care when they land is better off leaving it on.
    ///
    /// # Safety
    /// `fd` must be an open socket.
    #[no_mangle]
    pub unsafe extern "C" fn rill_sock_nodelay(fd: i64) -> i64 {
        let one: i32 = 1;
        let ok = setsockopt(
            fd as i32,
            IPPROTO_TCP,
            TCP_NODELAY,
            &one as *const i32 as *const u8,
            4,
        );
        i64::from(ok >= 0)
    }

    /// Waits until `fd` has something to say or `ms` milliseconds have gone
    /// by, and says which. The strand parks either way.
    ///
    /// Both are armed at once and whichever arrives first wakes the strand;
    /// the other is taken down afterwards. What it fired on is not asked of
    /// the poller — a peek at the socket answers it directly, and a peek
    /// cannot be wrong the way a race between two registrations can.
    ///
    /// # Safety
    /// `fd` must be an open socket; called from strand context.
    #[no_mangle]
    pub unsafe extern "C" fn rill_sock_wait(fd: i64, ms: i64) -> i8 {
        if peekable(fd as i32) {
            return 1;
        }
        if ms <= 0 {
            return 0;
        }
        // Waking is not the same as there being something to read, and the
        // difference is not rare. A descriptor is registered for reading and
        // for writing separately but is named in the poller by the descriptor
        // alone, so an edge that belongs to the other direction is delivered
        // here and looks exactly like the one being waited for: a socket
        // written to a moment ago is writable, and that edge wakes the read
        // this call is. Before this, `sock_wait(fd, 200)` immediately after a
        // `sock_write` on the same socket answered false in under a
        // millisecond — a deadline that had not remotely passed, reported as
        // if it had. So the wait is a loop against a deadline rather than one
        // sleep, and only the clock ends it.
        let deadline = rill_now_nanos() + ms * 1_000_000;
        let w = me();
        let g = (*w).current;
        loop {
            let left = (deadline - rill_now_nanos()) / 1_000_000;
            if left <= 0 {
                return peekable(fd as i32) as i8;
            }
            (*g).selected = -1;
            (*g).wait_ident = fd as usize + 1;
            let p = ensure_poller(w);
            IO_WAITERS.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
            (*g).on_io = true;
            let held = poller_arm_deadline(p, fd as i32, false, left, g);
            if held < 0 {
                (*g).on_io = false;
                IO_WAITERS.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
                return peekable(fd as i32) as i8;
            }
            block_current();
            poller_disarm(p, fd as i32, false, held, g);
            if peekable(fd as i32) {
                return 1;
            }
        }
    }

    /// Is there anything to read? A one-byte peek, which takes nothing.
    unsafe fn peekable(fd: i32) -> bool {
        let mut b: u8 = 0;
        let n = recv(fd, &raw mut b, 1, MSG_PEEK);
        // Zero means the peer has closed, which is something to read in the
        // sense that matters: the read that follows returns "" at once.
        n >= 0
    }

    /// Waits `ms` milliseconds without holding up the worker.
    ///
    /// # Safety
    /// Called from strand context.
    #[no_mangle]
    pub unsafe extern "C" fn rill_sleep_ms(ms: i64) {
        if ms <= 0 {
            return;
        }
        rill_flush_out();
        park_on_timer(ms);
    }

    /// Collects whatever the network has made ready and queues those strands.
    /// `wait` decides whether an idle scheduler should sleep here.
    pub(super) unsafe fn poll_network(w: *mut Worker, wait: bool) -> usize {
        let mut p = (*w).kq.load(core::sync::atomic::Ordering::Acquire);
        // A library's worker with nothing to do sleeps on its poller rather
        // than spinning: the library is idle most of the time, and three
        // workers spinning on the scheduler's counters slowed the host's
        // thread — which writes next to them — by half.
        let park = wait && !HOST_G.is_null();
        if !park && (p < 0 || IO_WAITERS.load(core::sync::atomic::Ordering::Acquire) == 0) {
            return 0;
        }
        if p < 0 {
            p = ensure_poller(w);
        }
        let mut ready: [*mut G; READY_BATCH] = [core::ptr::null_mut(); READY_BATCH];
        // A short sleep rather than an indefinite one: another worker may make
        // something runnable while this one is inside the call.
        if wait {
            (*w).asleep.store(true, core::sync::atomic::Ordering::SeqCst);
            // A worker that queued something for us a moment ago saw us awake and
            // sent no wakeup, so look once more before going to sleep on it —
            // at our own queue, and at the count of what sits on anyone's,
            // since a fresh strand is queued on its spawner's and the wake
            // for a stealer goes only to a worker already asleep. The fence
            // pairs with the one in `wake_a_stealer`: one side or the other
            // sees what the other wrote.
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            lock_runq(w);
            let pending = !(*w).runq.head.is_null();
            unlock_runq(w);
            if pending || (park && QUEUED.load(core::sync::atomic::Ordering::SeqCst) > 0) {
                (*w).asleep.store(false, core::sync::atomic::Ordering::Release);
                return 0;
            }
        }
        let n = poller_wait(p, if wait { 20 } else { 0 }, ready.as_mut_ptr());
        if wait {
            (*w).asleep.store(false, core::sync::atomic::Ordering::Release);
        }
        for i in 0..n {
            let g = ready[i];
            // A strand can be registered on two things at once — a socket and
            // a deadline — and both can come ready before it runs. Whoever
            // gets there first claims it; the second finds `on_io` already
            // false and leaves it alone, because queueing the same strand
            // twice corrupts the run queue and runs it twice.
            //
            // That flag is also what makes an event with no owner harmless. A
            // registration can outlive the wait that made it, and by the time
            // it is delivered the strand named in it may be parked on a
            // channel instead — where being made runnable is not a spurious
            // wake to be retried but a value that was never sent. It reads as
            // a null string, and it took down the server perhaps one arrival
            // in ten.
            let sel = core::sync::atomic::AtomicI64::from_ptr(&raw mut (*g).selected);
            if (*g).on_io
                && sel
                    .compare_exchange(
                        -1,
                        WOKEN_BY_POLLER,
                        core::sync::atomic::Ordering::AcqRel,
                        core::sync::atomic::Ordering::Acquire,
                    )
                    .is_ok()
            {
                (*g).on_io = false;
                IO_WAITERS.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
                make_ready(g);
            } else {
                STRAY_WAKES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            }
        }
        n
    }

    /// Opens a listening socket on `port`, or returns -1.
    ///
    /// # Safety
    /// Called from strand context.
    #[no_mangle]
    pub unsafe extern "C" fn rill_tcp_listen(port: i64) -> i64 {
        let fd = socket(AF_INET, SOCK_STREAM, 0);
        if fd < 0 {
            return -1;
        }
        let one: i32 = 1;
        setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one as *const i32 as *const u8, 4);
        let addr = sock_addr([0, 0, 0, 0], port);
        if bind(fd, &addr, 16) < 0 || listen(fd, 128) < 0 {
            close(fd);
            return -1;
        }
        set_nonblocking(fd);
        fd as i64
    }

    /// Waits for the next connection, parking rather than blocking the worker.
    ///
    /// # Safety
    /// `fd` must come from `rill_tcp_listen`; called from strand context.
    #[no_mangle]
    pub unsafe extern "C" fn rill_tcp_accept(fd: i64) -> i64 {
        loop {
            let c = accept(fd as i32, core::ptr::null_mut(), core::ptr::null_mut());
            if c >= 0 {
                set_nonblocking(c);
                no_sigpipe(c);
                return c as i64;
            }
            let e = errno();
            if e == EAGAIN {
                park_on_fd(fd as i32, false);
            } else if e != EINTR {
                return -1;
            }
        }
    }

    /// Parses a dotted-quad address. Names are not resolved: DNS lookup blocks,
    /// and nothing in the runtime may block a worker.
    unsafe fn parse_ipv4(s: *const u8) -> Option<[u8; 4]> {
        let bytes = core::slice::from_raw_parts(str_bytes(s), str_len_of(s));
        let mut out = [0u8; 4];
        let mut part = 0usize;
        let mut value: u32 = 0;
        let mut digits = 0;
        for &b in bytes {
            if b == b'.' {
                if digits == 0 || part >= 3 {
                    return None;
                }
                out[part] = value as u8;
                part += 1;
                value = 0;
                digits = 0;
            } else if b.is_ascii_digit() {
                value = value * 10 + (b - b'0') as u32;
                if value > 255 {
                    return None;
                }
                digits += 1;
            } else {
                return None;
            }
        }
        if part != 3 || digits == 0 {
            return None;
        }
        out[3] = value as u8;
        Some(out)
    }

    /// Connects to `host:port`, parking while the handshake completes.
    ///
    /// # Safety
    /// `host` must be a live string object; called from strand context.
    #[no_mangle]
    pub unsafe extern "C" fn rill_tcp_connect(host: *const u8, port: i64) -> i64 {
        let Some(ip) = parse_ipv4(host) else { return -1 };
        let fd = socket(AF_INET, SOCK_STREAM, 0);
        if fd < 0 {
            return -1;
        }
        set_nonblocking(fd);
        no_sigpipe(fd);
        let addr = sock_addr(ip, port);
        if connect(fd, &addr, 16) < 0 {
            let e = errno();
            if e != EINPROGRESS {
                close(fd);
                return -1;
            }
            park_on_fd(fd, true);
            // The handshake's result shows up in SO_ERROR.
            let mut err: i32 = 0;
            let mut len: u32 = 4;
            getsockopt(fd, SOL_SOCKET, SO_ERROR, &mut err as *mut i32 as *mut u8, &mut len);
            if err != 0 {
                // The wait above registered this descriptor, and the number is
                // about to go back to the operating system to hand out again.
                // A registration left behind is worse than a leak: the next
                // socket to be given this number, waited on by this same
                // strand, is taken for one already registered and is never
                // registered at all — so the wait never ends. Two refused
                // connections in a row were enough, and the second one hung
                // for ever.
                poller_forget(fd);
                close(fd);
                return -1;
            }
        }
        fd as i64
    }

    /// Reads whatever has arrived, parking until something has. An empty string
    /// means the peer closed the connection.
    ///
    /// # Safety
    /// `fd` must be an open socket; called from strand context.
    #[no_mangle]
    pub unsafe extern "C" fn rill_sock_read(fd: i64) -> *mut u8 {
        // One scratch block for every read on the one worker: it is only
        // used between `read` returning and the copy, with no park between,
        // so strands cannot see each other's bytes in it. Several workers
        // each take their own.
        let (buf, owned) = if rill_mt == 0 {
            if SOCK_SCRATCH.is_null() {
                SOCK_SCRATCH = malloc(READ_CHUNK);
            }
            (SOCK_SCRATCH, false)
        } else {
            (malloc(READ_CHUNK), true)
        };
        if buf.is_null() {
            out(b"rill: out of memory reading a socket\n");
            die();
        }
        loop {
            let n = read(fd as i32, buf, READ_CHUNK);
            if n >= 0 {
                let s = str_new(n as usize);
                core::ptr::copy_nonoverlapping(buf, s.add(16), n as usize);
                if owned {
                    free(buf);
                }
                return s;
            }
            let e = errno();
            if e == EAGAIN {
                park_on_fd(fd as i32, false);
            } else if e != EINTR {
                if owned {
                    free(buf);
                }
                return str_new(0);
            }
        }
    }

    static mut SOCK_SCRATCH: *mut u8 = core::ptr::null_mut();

    /// Writes a whole string, parking whenever the socket is full.
    ///
    /// # Safety
    /// `fd` must be an open socket and `s` a live string; called from strand
    /// context.
    #[no_mangle]
    pub unsafe extern "C" fn rill_sock_write(fd: i64, s: *const u8) -> i64 {
        let total = str_len_of(s);
        if rill_mt == 0 && (fd as usize) < WBUFS_LEN {
            let b = WBUFS.add(fd as usize);
            if !(*b).data.is_null() {
                // A message wider than the buffer goes straight out, after
                // what is already waiting; anything else is queued, and the
                // queue is written when it fills.
                if total >= WBUF_CAP {
                    if rill_sock_flush(fd) < 0 || !send_all(fd as i32, str_bytes(s), total) {
                        return -1;
                    }
                    return total as i64;
                }
                if (*b).len + total > WBUF_CAP && rill_sock_flush(fd) < 0 {
                    return -1;
                }
                if (*b).len == 0 && total > 0 {
                    PENDING += 1;
                }
                core::ptr::copy_nonoverlapping(str_bytes(s), (*b).data.add((*b).len), total);
                (*b).len += total;
                return total as i64;
            }
        }
        if send_all(fd as i32, str_bytes(s), total) { total as i64 } else { -1 }
    }

    /// Writes all of `n` bytes, parking whenever the socket is full.
    unsafe fn send_all(fd: i32, p: *const u8, n: usize) -> bool {
        let mut sent = 0usize;
        while sent < n {
            let k = sock_send(fd, p.add(sent), n - sent);
            if k > 0 {
                sent += k as usize;
                continue;
            }
            let e = errno();
            if e == EAGAIN {
                park_on_fd(fd, true);
            } else if e != EINTR {
                return false;
            }
        }
        true
    }

    // ---- buffered writing -------------------------------------------------
    //
    // `sock_buffered(fd)` gives a socket a 64 KB write buffer: `sock_write`
    // then queues rather than calls `write`, and the queue goes out in one
    // call when it fills, on `sock_flush`, when the strand parks — on a read,
    // a channel, a timer, anything — and when the socket closes. A program
    // that writes many small pieces before it waits for anything pays one
    // system call for all of them instead of one each; a program that writes
    // and waits, like an echo, gains nothing and loses nothing, because the
    // wait flushes. Nothing is buffered on several workers: the table is one
    // worker's.

    const WBUF_CAP: usize = 64 * 1024;
    struct WBuf {
        data: *mut u8,
        len: usize,
    }
    static mut WBUFS: *mut WBuf = core::ptr::null_mut();
    static mut WBUFS_LEN: usize = 0;
    /// How many buffers hold anything, so a park with nothing pending costs
    /// one compare.
    static mut PENDING: usize = 0;
    /// Set while the pending buffers are being written out, since writing
    /// one may park, and a park flushes.
    static mut FLUSHING: bool = false;

    unsafe fn wbuf(fd: i32) -> *mut WBuf {
        let fd = fd as usize;
        if fd >= WBUFS_LEN {
            let mut want = if WBUFS_LEN == 0 { 64 } else { WBUFS_LEN };
            while want <= fd {
                want *= 2;
            }
            let fresh = calloc(want, core::mem::size_of::<WBuf>()) as *mut WBuf;
            if fresh.is_null() {
                out(b"rill: out of memory buffering a socket\n");
                die();
            }
            if !WBUFS.is_null() {
                core::ptr::copy_nonoverlapping(WBUFS, fresh, WBUFS_LEN);
                free(WBUFS as *mut u8);
            }
            WBUFS = fresh;
            WBUFS_LEN = want;
        }
        WBUFS.add(fd)
    }

    /// # Safety
    /// `fd` must be an open socket; called from strand context.
    #[no_mangle]
    pub unsafe extern "C" fn rill_sock_buffered(fd: i64) {
        if rill_mt != 0 || fd < 0 {
            return;
        }
        let b = wbuf(fd as i32);
        if (*b).data.is_null() {
            (*b).data = malloc(WBUF_CAP);
            (*b).len = 0;
            if (*b).data.is_null() {
                out(b"rill: out of memory buffering a socket\n");
                die();
            }
        }
    }

    /// Writes out what `sock_write` has queued on `fd`: the number of bytes
    /// that went, 0 for nothing waiting or an unbuffered socket, -1 on a
    /// socket error.
    ///
    /// # Safety
    /// `fd` must be an open socket; called from strand context.
    #[no_mangle]
    pub unsafe extern "C" fn rill_sock_flush(fd: i64) -> i64 {
        if rill_mt != 0 || fd < 0 || (fd as usize) >= WBUFS_LEN {
            return 0;
        }
        let b = WBUFS.add(fd as usize);
        let n = (*b).len;
        if (*b).data.is_null() || n == 0 {
            return 0;
        }
        (*b).len = 0;
        PENDING -= 1;
        if send_all(fd as i32, (*b).data, n) { n as i64 } else { -1 }
    }

    /// Every buffer with something in it, written out. Called before a strand
    /// parks, so nothing a program wrote waits on the program.
    pub(super) unsafe fn flush_pending() {
        if PENDING == 0 || FLUSHING || rill_mt != 0 {
            return;
        }
        FLUSHING = true;
        let mut fd = 0usize;
        while fd < WBUFS_LEN && PENDING > 0 {
            let b = WBUFS.add(fd);
            if !(*b).data.is_null() && (*b).len > 0 {
                rill_sock_flush(fd as i64);
            }
            fd += 1;
        }
        FLUSHING = false;
    }

    /// The same at exit, where there is no strand to park: whatever the
    /// kernel takes at once goes, the rest is lost with the process.
    pub(super) unsafe fn flush_pending_at_exit() {
        if PENDING == 0 || rill_mt != 0 {
            return;
        }
        let mut fd = 0usize;
        while fd < WBUFS_LEN {
            let b = WBUFS.add(fd);
            if !(*b).data.is_null() && (*b).len > 0 {
                sock_send(fd as i32, (*b).data, (*b).len);
                (*b).len = 0;
            }
            fd += 1;
        }
        PENDING = 0;
    }

    /// Shuts a connection down in both directions without closing it, which
    /// is the only way one strand can get another out of a read.
    ///
    /// The reference says why closing is not that way: the reader is parked on
    /// the descriptor inside the poller, closing it from elsewhere does not
    /// wake it, and the number can be handed to the next connection while the
    /// old registration is still armed. `shutdown` is different in exactly the
    /// place that matters. It acts on the connection rather than on the
    /// descriptor, so the number stays this program's until it closes it, and
    /// the socket becomes readable-at-end-of-file at once — which is a
    /// readiness event, so the poller wakes the parked strand and its
    /// `sock_read` answers `""`. The reader then closes the descriptor itself,
    /// on its own strand, where it has always been safe to.
    ///
    /// A server that has to put a limit on how long it will hold a connection,
    /// or drop every one of them on the way down, now has something to call.
    ///
    /// # Safety
    /// `fd` must be an open socket.
    #[no_mangle]
    pub unsafe extern "C" fn rill_sock_shutdown(fd: i64) -> i64 {
        // SHUT_RDWR is 2 on every Unix and on Windows, one of the few socket
        // constants that never got renumbered.
        shutdown(fd as i32, 2) as i64
    }

    /// # Safety
    /// `fd` must be an open socket.
    #[no_mangle]
    pub unsafe extern "C" fn rill_sock_close(fd: i64) {
        if rill_mt == 0 && fd >= 0 && (fd as usize) < WBUFS_LEN {
            let b = WBUFS.add(fd as usize);
            if !(*b).data.is_null() {
                rill_sock_flush(fd);
                free((*b).data);
                (*b).data = core::ptr::null_mut();
            }
        }
        poller_forget(fd as i32);
        close(fd as i32);
    }

}

// ---- bare metal -----------------------------------------------------------

/// The reference count is atomic on a host running several workers. ARMv7-M
/// has no 64-bit atomic instruction to be atomic with, and a Cortex-M build is
/// single-worker by construction — `rill_mt` is never 1 there — so the arm
/// that would have synchronized is a plain load and store, and the type it
/// went through is this one.
#[cfg(target_os = "none")]
#[repr(transparent)]
struct RcAtomic(core::cell::UnsafeCell<i64>);

#[cfg(target_os = "none")]
impl RcAtomic {
    #[inline(always)]
    unsafe fn fetch_add(&self, d: i64, _order: core::sync::atomic::Ordering) -> i64 {
        let p = self.0.get();
        let old = *p;
        *p = old + d;
        old
    }

    #[inline(always)]
    unsafe fn fetch_sub(&self, d: i64, _order: core::sync::atomic::Ordering) -> i64 {
        let p = self.0.get();
        let old = *p;
        *p = old - d;
        old
    }
}

#[cfg(not(target_os = "none"))]
use core::sync::atomic::AtomicI64 as RcAtomic;
