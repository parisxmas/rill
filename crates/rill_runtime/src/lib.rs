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
    fn exit(code: i32) -> !;
    fn malloc(size: usize) -> *mut u8;
    fn calloc(count: usize, size: usize) -> *mut u8;
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
    unsafe { abort() }
}

/// Satisfies the linker when the staticlib is built with unwinding enabled;
/// nothing here ever unwinds.
#[no_mangle]
pub extern "C" fn rust_eh_personality() {}

fn out(bytes: &[u8]) {
    unsafe {
        write(1, bytes.as_ptr(), bytes.len());
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
unsafe fn sb_grow(p: *mut u8, len: usize, cap: usize, want: usize) {
    let fresh = rill_alloc(want as u64);
    // The block is raw bytes, not an object with a count: the allocation
    // ledger must not see it, or every live builder would look like a leak.
    live_add(-1);
    let old = sb_data(p);
    if !old.is_null() {
        core::ptr::copy_nonoverlapping(old, fresh, len);
        live_add(1);
        rill_free_sized(old, cap as u64);
    }
    *(p as *mut *mut u8).add(1) = fresh;
    *(p as *mut i64).add(3) = want as i64;
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
            rill_free_sized(d, *(p as *const i64).add(3) as u64);
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
#[inline]
fn dec(v: i64, buf: &mut [u8; 24]) -> usize {
    let mut i = 24;
    // Negated rather than made positive, so that i64::MIN works.
    let mut n = if v < 0 { v } else { -v };
    loop {
        i -= 1;
        buf[i] = b'0' + (-(n % 10)) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    if v < 0 {
        i -= 1;
        buf[i] = b'-';
    }
    i
}

/// # Safety
/// Returns a new owned string.
#[no_mangle]
pub unsafe extern "C" fn rill_int_to_str(v: i64) -> *mut u8 {
    let mut buf = [0u8; 24];
    let i = dec(v, &mut buf);
    let n = 24 - i;
    let p = str_new(n);
    copy_small(buf.as_ptr().add(i), p.add(16), n);
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
    let hay = str_bytes(s);
    let nee = str_bytes(needle);
    // Jump to each place the first byte occurs and compare from there. The
    // jumping is the vectorized part and it is where the time goes: a needle's
    // first byte is rare in ordinary text, so most of the haystack is crossed
    // sixteen bytes at a time and never looked at again.
    let mut i = start;
    while i + m <= n {
        let hit = memchr(hay.add(i), *nee as i32, n - m - i + 1);
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
        abort();
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
    let fd = open(c, 0);
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

/// Writes (creates/truncates) a whole file; returns success.
///
/// # Safety
/// Both must be live string objects.
#[no_mangle]
pub unsafe extern "C" fn rill_write_file(path: *const u8, content: *const u8) -> i8 {
    IO_OK = 1;
    let c = cstr(path);
    // O_WRONLY | O_CREAT | O_TRUNC, mode 0644
    let fd = open(c, 0x1 | 0x200 | 0x400, 0o644 as i32);
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

// Standard input is read in blocks and handed out a line at a time. A
// syscall per byte is what this used to be, and on a large input that was all
// the program did; nothing else reads this descriptor, so holding what was
// read ahead is safe.
const IN_CAP: usize = 1 << 16;
static mut IN_BUF: *mut u8 = core::ptr::null_mut();
static mut IN_POS: usize = 0;
static mut IN_LEN: usize = 0;

/// The next byte of standard input, or -1 at the end of it.
unsafe fn in_byte() -> i32 {
    if IN_POS == IN_LEN {
        if IN_BUF.is_null() {
            IN_BUF = malloc(IN_CAP);
        }
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
    let mut cap = 128usize;
    let mut buf = malloc(cap);
    let mut n = 0usize;
    loop {
        let c = in_byte();
        if c < 0 {
            if n == 0 {
                IO_OK = 0;
            }
            break;
        }
        let b = c as u8;
        if b == b'\n' {
            break;
        }
        if n == cap {
            let bigger = malloc(cap * 2);
            core::ptr::copy_nonoverlapping(buf, bigger, n);
            free(buf);
            buf = bigger;
            cap *= 2;
        }
        *buf.add(n) = b;
        n += 1;
    }
    let p = str_new(n);
    core::ptr::copy_nonoverlapping(buf, p.add(16), n);
    free(buf);
    p
}

/// # Safety
/// Terminates the process.
#[no_mangle]
pub unsafe extern "C" fn rill_exit(code: i64) -> ! {
    exit(code as i32)
}

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
#[no_mangle]
pub unsafe extern "C" fn rill_buf_new(n: i64, size: i64) -> *mut u8 {
    if n < 0 {
        rill_buf_oob(n, 0);
    }
    let bytes = (n as u64).saturating_mul(size as u64);
    let p = rill_alloc(16 + bytes);
    *(p as *mut i64) = 1;
    *(p as *mut i64).add(1) = n;
    let mut i = 0u64;
    while i < bytes {
        *p.add(16 + i as usize) = 0;
        i += 1;
    }
    p
}

/// # Safety
/// `p` must be a live buffer whose elements are `size` bytes.
#[no_mangle]
pub unsafe extern "C" fn rill_buf_release(p: *mut u8, size: i64) {
    let n = *(p as *const i64).add(1);
    if rc_dec(p as *mut i64) == 0 {
        rill_free_sized(p, 16 + (n as u64) * (size as u64));
    }
}

/// Nothing sensible follows an index off the end, so this ends the program
/// the way an array bound violation should.
#[no_mangle]
pub extern "C" fn rill_buf_oob(i: i64, len: i64) -> ! {
    unsafe {
        out(b"rill: buffer index ");
        out_dec(i);
        out(b" out of range, length ");
        out_dec(len);
        out(b"\n");
        abort()
    }
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
static mut LIVE_ALLOCS: i64 = 0;

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
        (*(p as *const core::sync::atomic::AtomicI64))
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
}

#[inline(always)]
unsafe fn live_add(delta: i64) {
    if rill_mt == 0 {
        LIVE_ALLOCS += delta;
    } else {
        (*(&raw const LIVE_ALLOCS as *const core::sync::atomic::AtomicI64))
            .fetch_add(delta, core::sync::atomic::Ordering::Relaxed);
    }
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
        (*(p as *const core::sync::atomic::AtomicI64))
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
struct Pool {
    free: [*mut u8; POOL_CLASSES],
    next: usize,
    end: usize,
}

const POOL_INIT: Pool = Pool {
    free: [core::ptr::null_mut(); POOL_CLASSES],
    next: 0,
    end: 0,
};

static mut POOLS: [Pool; MAX_WORKERS] = [POOL_INIT; MAX_WORKERS];

/// The pool belonging to the calling thread. A single-worker program skips
/// the thread-local lookup, which is the case that has to stay fast.
#[inline(always)]
unsafe fn my_pool() -> *mut Pool {
    if rill_mt == 0 {
        &raw mut POOLS[0]
    } else {
        &raw mut POOLS[(*me()).index]
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
        let pool = my_pool();
        let head = (*pool).free[class];
        if !head.is_null() {
            (*pool).free[class] = *(head as *const *mut u8);
            return head;
        }
        let bytes = (class + 1) * 16;
        if (*pool).next + bytes > (*pool).end {
            let p = mmap(core::ptr::null_mut(), POOL_CHUNK, PROT_RW, MAP_PRIVATE_ANON, -1, 0);
            if p as isize == -1 {
                out(b"rill: out of memory\n");
                abort();
            }
            (*pool).next = p as usize;
            (*pool).end = p as usize + POOL_CHUNK;
        }
        let p = (*pool).next as *mut u8;
        (*pool).next += bytes;
        return p;
    }
    let p = malloc(size as usize);
    if p.is_null() {
        out(b"rill: out of memory\n");
        abort();
    }
    p
}

/// # Safety
/// `ptr` must come from `rill_alloc` with this exact `size`, freed once.
#[no_mangle]
pub unsafe extern "C" fn rill_free_sized(ptr: *mut u8, size: u64) {
    live_add(-1);
    if let Some(class) = pool_class(size) {
        let pool = my_pool();
        *(ptr as *mut *mut u8) = (*pool).free[class];
        (*pool).free[class] = ptr;
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
    let mut buf = [0u8; 128];
    let n = if ABANDONED == 0 {
        snprintf(buf.as_mut_ptr(), 127, b"live allocations: %lld\n\0".as_ptr(), LIVE_ALLOCS)
    } else {
        snprintf(
            buf.as_mut_ptr(),
            127,
            b"live allocations: %lld (%lld strand(s) were still running at exit)\n\0".as_ptr(),
            LIVE_ALLOCS,
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
const MAP_PRIVATE_ANON: i32 = 0x0002 | 0x1000;

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
#[cfg(target_family = "wasm")]
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
#[cfg(target_family = "wasm")]
unsafe fn pthread_mutex_init(_m: *mut PthreadMutex, _attr: *const u8) -> i32 {
    0
}

#[cfg(target_family = "wasm")]
unsafe fn pthread_mutex_lock(_m: *mut PthreadMutex) -> i32 {
    0
}

#[cfg(target_family = "wasm")]
unsafe fn pthread_mutex_unlock(_m: *mut PthreadMutex) -> i32 {
    0
}

#[cfg(target_family = "wasm")]
unsafe fn pthread_create(
    _thread: *mut usize,
    _attr: *const u8,
    _start: extern "C" fn(*mut u8) -> *mut u8,
    _arg: *mut u8,
) -> i32 {
    -1
}

#[cfg(target_family = "wasm")]
unsafe fn pthread_key_create(_key: *mut usize, _destructor: *const u8) -> i32 {
    0
}

#[cfg(target_family = "wasm")]
unsafe fn pthread_setspecific(_key: usize, _value: *const u8) -> i32 {
    0
}

#[cfg(target_family = "wasm")]
unsafe fn pthread_getspecific(_key: usize) -> *mut u8 {
    core::ptr::null_mut()
}

/// There is no other thread to yield to.
#[cfg(target_family = "wasm")]
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
#[cfg(target_family = "wasm")]
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
        abort();
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
        abort();
    }
    let mut was: u32 = 0;
    VirtualProtect(p, PAGE, PAGE_NOACCESS, &raw mut was);
    p
}

/// Wasm's call stack is the engine's, not memory this program can address, so
/// there is no such thing as a second one to run a strand on. Reaching here
/// means the compile-time refusal was not applied.
#[cfg(target_family = "wasm")]
unsafe fn alloc_stack(_usable: usize) -> *mut u8 {
    out(b"rill: strands are not available on wasm\n");
    abort();
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

#[cfg(target_family = "wasm")]
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
    /// Queue node for a plain send or receive, which never needs more than one.
    waiter: Waiter,
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
    /// Set while this worker is asleep inside `kevent`, so that another worker
    /// only pays for a wakeup syscall when there is really someone to wake.
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
    asleep: core::sync::atomic::AtomicBool::new(false),
};

static mut WORKERS: [Worker; MAX_WORKERS] = [WORKER_INIT; MAX_WORKERS];
/// Assigned by `rill_rt_run` before anything reads it; zero here keeps the
/// table out of the binary.
static mut NWORKERS: usize = 0;
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
#[cfg(target_family = "wasm")]
unsafe extern "C" fn ctx_switch(_old: *mut Ctx, _new: *const Ctx) {
    out(b"rill: strands are not available on wasm\n");
    abort();
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
            abort();
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
        abort();
    }
    let total = MAIN_STACK_SIZE + PAGE;
    let stack = alloc_stack(MAIN_STACK_SIZE);
    (*g).saved = core::ptr::null_mut();
    (*g).saved_cap = 0;
    (*g).is_main = true;
    init_g(g, code, core::ptr::null_mut());
    (*g).started = true;
    (*g).ctx.regs[12] = ((stack as usize + total) & !15) as u64;
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
            abort();
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
    let w = me();
    let g = (*w).current;
    ctx_switch(&mut (*g).ctx, &(*w).sched_ctx);
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
            if QUEUED.load(core::sync::atomic::Ordering::Acquire) > 0 {
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
        lock_runq(w);
        let mut g = (*w).runq.pop();
        unlock_runq(w);
        if g.is_null() && rill_mt != 0 {
            // Nothing of our own: take unstarted work from another worker.
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
            if (*MAIN_G).dead {
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
            if idle_rounds > 1000 {
                // Nothing is executing and no queue holds work, round after
                // round: nothing can ever wake the parked strands.
                out(b"rill: deadlock - all strands are blocked\n");
                abort();
            }
            sched_yield();
            continue;
        }
        if !(*g).started {
            // First run: adopt this worker's execution stack.
            (*g).started = true;
            (*g).home = (*w).index;
            (*g).ctx.regs[12] = (*w).exec_top as u64;
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
    code();
}

#[no_mangle]
pub unsafe extern "C" fn rill_rt_run(code: extern "C" fn(*mut u8)) {
    NWORKERS = requested_threads();
    if NWORKERS > 1 && rill_parallel_build == 0 {
        // The generated code updates reference counts without synchronizing,
        // which is what makes single-worker programs fast. Running it on
        // several workers would be unsound, so say so and stay on one.
        let msg = b"rill: RILL_THREADS ignored - rebuild with `rill build --parallel`\n";
        write(2, msg.as_ptr(), msg.len());
        NWORKERS = 1;
    }
    pthread_mutex_init(&raw mut CHAN_LOCK, core::ptr::null());
    for i in 0..NWORKERS {
        let base = alloc_stack(EXEC_STACK_SIZE);
        WORKERS[i].index = i;
        WORKERS[i].exec_top = (base as usize + EXEC_STACK_SIZE + PAGE) & !15;
        // The statics start zeroed, so the parts that are not zero at rest
        // are set here: a usable mutex, and "no kqueue yet".
        pthread_mutex_init(&raw mut WORKERS[i].lock, core::ptr::null());
        WORKERS[i].kq.store(-1, core::sync::atomic::Ordering::Release);
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
}

const WAITER_INIT: Waiter = Waiter {
    next: core::ptr::null_mut(),
    g: core::ptr::null_mut(),
    item: [0; 2],
    case: 0,
    chan: core::ptr::null_mut(),
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

    /// The next waiter whose strand is still up for grabs, dropping any
    /// that another channel already claimed.
    unsafe fn pop_live(&mut self) -> *mut Waiter {
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
            if (*(*w).g).selected < 0 {
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

/// Hands a waiting strand its result and makes it runnable.
unsafe fn claim(w: *mut Waiter) {
    (*(*w).g).selected = (*w).case;
    make_ready((*w).g);
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
    let s = (*c).sendq.pop_live();
    if !s.is_null() {
        let item = (*s).item;
        chan_push(c, item[0], item[1]);
        claim(s);
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
        Chan { rc: 1, cap, len: 0, head: 0, sendq: WQueue::new(), recvq: WQueue::new() },
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

/// Sends one value, parking until it can be handed over or buffered.
///
/// # Safety
/// `p` must be a live channel; called from strand context.
#[no_mangle]
pub unsafe extern "C" fn rill_chan_send(p: *mut u8, a: u64, b: u64) {
    let c = p as *mut Chan;
    lock();
    // A waiting receiver takes it directly, whether or not there is a buffer.
    let r = (*c).recvq.pop_live();
    if !r.is_null() {
        (*r).item = [a, b];
        claim(r);
        unlock();
        return;
    }
    if (*c).len < (*c).cap {
        chan_push(c, a, b);
        unlock();
        return;
    }
    let g = (*me()).current;
    (*g).selected = -1;
    (*g).waiter = Waiter { next: core::ptr::null_mut(), g, item: [a, b], case: 0, chan: c };
    (*c).sendq.push(&raw mut (*g).waiter);
    unlock();
    block_current();
}

/// Receives one value, parking until a sender or a buffered value arrives.
///
/// # Safety
/// `p` must be a live channel; called from strand context.
#[no_mangle]
pub unsafe extern "C" fn rill_chan_recv(p: *mut u8) -> TwoWords {
    let c = p as *mut Chan;
    lock();
    if (*c).len > 0 {
        let out = chan_take_buffered(c);
        unlock();
        return out;
    }
    let s = (*c).sendq.pop_live();
    if !s.is_null() {
        let item = (*s).item;
        claim(s);
        unlock();
        return TwoWords { a: item[0], b: item[1] };
    }
    let g = (*me()).current;
    (*g).selected = -1;
    (*g).waiter =
        Waiter { next: core::ptr::null_mut(), g, item: [0, 0], case: 0, chan: c };
    (*c).recvq.push(&raw mut (*g).waiter);
    unlock();
    block_current();
    let item = (*g).waiter.item;
    TwoWords { a: item[0], b: item[1] }
}

// ---- select ---------------------------------------------------------------

const SEL_RECV: i64 = 0;

/// One arm of a `select`, filled in by generated code.
#[repr(C)]
pub struct SelectCase {
    pub kind: i64,
    pub chan: *mut u8,
    pub a: u64,
    pub b: u64,
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
    lock();
    for i in 0..n {
        let c = &*cases.add(i as usize);
        let ch = c.chan as *mut Chan;
        if c.kind == SEL_RECV {
            if (*ch).len > 0 {
                let v = chan_take_buffered(ch);
                unlock();
                *slot = v.a;
                *slot.add(1) = v.b;
                return i;
            }
            let s = (*ch).sendq.pop_live();
            if !s.is_null() {
                let item = (*s).item;
                claim(s);
                unlock();
                *slot = item[0];
                *slot.add(1) = item[1];
                return i;
            }
        } else {
            let r = (*ch).recvq.pop_live();
            if !r.is_null() {
                (*r).item = [c.a, c.b];
                claim(r);
                unlock();
                return i;
            }
            if (*ch).len < (*ch).cap {
                chan_push(ch, c.a, c.b);
                unlock();
                return i;
            }
        }
    }
    if has_default != 0 {
        unlock();
        return -1;
    }

    // Nothing is ready: queue up on every channel at once. The nodes live on
    // the heap because a parked strand's stack is copied away.
    let ws = malloc(n as usize * core::mem::size_of::<Waiter>()) as *mut Waiter;
    if ws.is_null() {
        out(b"rill: out of memory in select\n");
        abort();
    }
    let g = (*me()).current;
    (*g).selected = -1;
    for i in 0..n {
        let c = &*cases.add(i as usize);
        let ch = c.chan as *mut Chan;
        let w = ws.add(i as usize);
        core::ptr::write(
            w,
            Waiter { next: core::ptr::null_mut(), g, item: [c.a, c.b], case: i, chan: ch },
        );
        if c.kind == SEL_RECV {
            (*ch).recvq.push(w);
        } else {
            (*ch).sendq.push(w);
        }
    }
    unlock();
    block_current();

    lock();
    let idx = (*g).selected;
    let item = (*ws.add(idx as usize)).item;
    // Whatever is still queued elsewhere has to go before the nodes die.
    for i in 0..n {
        let w = ws.add(i as usize);
        let ch = (*w).chan;
        if (*cases.add(i as usize)).kind == SEL_RECV {
            (*ch).recvq.remove(w);
        } else {
            (*ch).sendq.remove(w);
        }
    }
    unlock();
    free(ws as *mut u8);
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

const AF_INET: i32 = 2;
const SOCK_STREAM: i32 = 1;
const SOL_SOCKET: i32 = 0xffff;
const SO_REUSEADDR: i32 = 0x0004;
const SO_ERROR: i32 = 0x1007;
const F_SETFL: i32 = 4;
const O_NONBLOCK: i32 = 0x0004;// ---- sockets ---------------------------------------------------------------
//
// BSD sockets and kqueue, which is to say: not Windows. Winsock has the same
// shape and different names, and its waiting is done with completion ports
// rather than by asking which handles are ready — so the port of this section
// is a rewrite rather than a rename, and it is not written yet. A program
// that opens no socket links and runs without it.
#[cfg(unix)]
use net::{poll_network, wake_a_stealer, wake_worker, IO_WAITERS};

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

/// The same silence on wasm, for a different reason: wasip1 has no sockets to
/// offer and no worker to wake if it had.
#[cfg(target_family = "wasm")]
unsafe fn wake_worker(_w: *mut Worker) {}
#[cfg(target_family = "wasm")]
unsafe fn wake_a_stealer() {}
#[cfg(target_family = "wasm")]
unsafe fn poll_network(_w: *mut Worker, _block: bool) -> i32 {
    0
}
#[cfg(target_family = "wasm")]
static IO_WAITERS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

#[cfg(unix)]
mod net {
    use super::*;

    const EAGAIN: i32 = 35;
    const EINPROGRESS: i32 = 36;
    const EINTR: i32 = 4;

    const EVFILT_READ: i16 = -1;
    const EVFILT_WRITE: i16 = -2;
    /// A filter the program triggers itself, used to wake a worker asleep in
    /// `kevent` the moment another worker queues work for it.
    const EVFILT_USER: i16 = -10;
    const EV_ADD: u16 = 0x0001;
    const EV_ONESHOT: u16 = 0x0010;
    const EV_CLEAR: u16 = 0x0020;
    const NOTE_TRIGGER: u32 = 0x0100_0000;
    /// Identifier of that wakeup event. Filters have separate ident spaces, so
    /// this cannot collide with a file descriptor.
    const WAKE_IDENT: usize = 0;

    const READ_CHUNK: usize = 64 * 1024;

    extern "C" {
        fn socket(domain: i32, ty: i32, protocol: i32) -> i32;
        fn bind(fd: i32, addr: *const SockAddrIn, len: u32) -> i32;
        fn listen(fd: i32, backlog: i32) -> i32;
        fn accept(fd: i32, addr: *mut u8, len: *mut u32) -> i32;
        fn connect(fd: i32, addr: *const SockAddrIn, len: u32) -> i32;
        fn setsockopt(fd: i32, level: i32, name: i32, val: *const u8, len: u32) -> i32;
        fn getsockopt(fd: i32, level: i32, name: i32, val: *mut u8, len: *mut u32) -> i32;
        fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        fn kqueue() -> i32;
        fn kevent(
            kq: i32,
            changes: *const KEvent,
            nchanges: i32,
            events: *mut KEvent,
            nevents: i32,
            timeout: *const TimeSpec,
        ) -> i32;
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

    #[repr(C)]
    struct SockAddrIn {
        len: u8,
        family: u8,
        port: u16,
        addr: [u8; 4],
        zero: [u8; 8],
    }

    #[repr(C)]
    struct KEvent {
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

    /// Strands currently parked on a file descriptor. While this is non-zero
    /// an idle scheduler waits in kqueue instead of declaring a deadlock.
    pub(super) static IO_WAITERS: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

    unsafe fn errno() -> i32 {
        *__error()
    }

    /// Opens this worker's poller on first use. Only the owning worker calls it,
    /// so the lazy creation needs no lock.
    unsafe fn ensure_kqueue(w: *mut Worker) -> i32 {
        let existing = (*w).kq.load(core::sync::atomic::Ordering::Acquire);
        if existing >= 0 {
            return existing;
        }
        let kq = kqueue();
        if kq < 0 {
            out(b"rill: cannot create kqueue\n");
            abort();
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

    /// Cuts short the sleep of one specific worker. Without this, work queued by
    /// another worker would sit until that sleep timed out.
    pub(super) unsafe fn wake_worker(w: *mut Worker) {
        if !(*w).asleep.load(core::sync::atomic::Ordering::Acquire) {
            return;
        }
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

    /// Wakes one sleeping worker so it can come and steal. Fresh strands sit on
    /// the queue of whoever spawned them, and a worker asleep on the network would
    /// otherwise not find out there is anything to take.
    pub(super) unsafe fn wake_a_stealer() {
        for i in 0..NWORKERS {
            let v = &raw mut WORKERS[i];
            if (*v).asleep.load(core::sync::atomic::Ordering::Acquire) {
                wake_worker(v);
                return;
            }
        }
    }

    unsafe fn set_nonblocking(fd: i32) {
        fcntl(fd, F_SETFL, O_NONBLOCK);
    }

    /// Parks the current strand until `fd` is readable or writable. The
    /// registration is one-shot, so nothing has to be unregistered afterwards.
    unsafe fn park_on_fd(fd: i32, writable: bool) {
        let w = me();
        let g = (*w).current;
        (*g).selected = -1;
        let kq = ensure_kqueue(w);
        let ev = KEvent {
            ident: fd as usize,
            filter: if writable { EVFILT_WRITE } else { EVFILT_READ },
            flags: EV_ADD | EV_ONESHOT,
            fflags: 0,
            data: 0,
            udata: g as *mut u8,
        };
        IO_WAITERS.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
        if kevent(kq, &ev, 1, core::ptr::null_mut(), 0, core::ptr::null()) < 0 {
            IO_WAITERS.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
            return; // fall back to spinning on the syscall
        }
        block_current();
    }

    /// Collects whatever the network has made ready and queues those strands.
    /// `wait` decides whether an idle scheduler should sleep here.
    pub(super) unsafe fn poll_network(w: *mut Worker, wait: bool) -> usize {
        let kq = (*w).kq.load(core::sync::atomic::Ordering::Acquire);
        if kq < 0 || IO_WAITERS.load(core::sync::atomic::Ordering::Acquire) == 0 {
            return 0;
        }
        let mut events: [KEvent; 32] = core::mem::zeroed();
        // A short sleep rather than an indefinite one: another worker may make
        // something runnable while this one is inside the call.
        let ts = TimeSpec { sec: 0, nsec: if wait { 20_000_000 } else { 0 } };
        if wait {
            (*w).asleep.store(true, core::sync::atomic::Ordering::Release);
            // A worker that queued something for us a moment ago saw us awake and
            // sent no wakeup, so look once more before going to sleep on it.
            lock_runq(w);
            let pending = !(*w).runq.head.is_null();
            unlock_runq(w);
            if pending {
                (*w).asleep.store(false, core::sync::atomic::Ordering::Release);
                return 0;
            }
        }
        let n = kevent(kq, core::ptr::null(), 0, events.as_mut_ptr(), 32, &ts);
        if wait {
            (*w).asleep.store(false, core::sync::atomic::Ordering::Release);
        }
        if n <= 0 {
            return 0;
        }
        for i in 0..n as usize {
            let g = events[i].udata as *mut G;
            if g.is_null() {
                continue;
            }
            IO_WAITERS.fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
            (*g).selected = 0;
            lock();
            make_ready(g);
            unlock();
        }
        n as usize
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
        let addr = SockAddrIn {
            len: 16,
            family: AF_INET as u8,
            port: (port as u16).to_be(),
            addr: [0, 0, 0, 0],
            zero: [0; 8],
        };
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
        let addr = SockAddrIn {
            len: 16,
            family: AF_INET as u8,
            port: (port as u16).to_be(),
            addr: ip,
            zero: [0; 8],
        };
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
        let buf = malloc(READ_CHUNK);
        if buf.is_null() {
            out(b"rill: out of memory reading a socket\n");
            abort();
        }
        loop {
            let n = read(fd as i32, buf, READ_CHUNK);
            if n >= 0 {
                let s = str_new(n as usize);
                core::ptr::copy_nonoverlapping(buf, s.add(16), n as usize);
                free(buf);
                return s;
            }
            let e = errno();
            if e == EAGAIN {
                park_on_fd(fd as i32, false);
            } else if e != EINTR {
                free(buf);
                return str_new(0);
            }
        }
    }

    /// Writes a whole string, parking whenever the socket is full.
    ///
    /// # Safety
    /// `fd` must be an open socket and `s` a live string; called from strand
    /// context.
    #[no_mangle]
    pub unsafe extern "C" fn rill_sock_write(fd: i64, s: *const u8) -> i64 {
        let total = str_len_of(s);
        let mut sent = 0usize;
        while sent < total {
            let n = write(fd as i32, str_bytes(s).add(sent), total - sent);
            if n > 0 {
                sent += n as usize;
                continue;
            }
            let e = errno();
            if e == EAGAIN {
                park_on_fd(fd as i32, true);
            } else if e != EINTR {
                return -1;
            }
        }
        sent as i64
    }

    /// # Safety
    /// `fd` must be an open socket.
    #[no_mangle]
    pub unsafe extern "C" fn rill_sock_close(fd: i64) {
        close(fd as i32);
    }

}
