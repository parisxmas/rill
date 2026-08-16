const std = @import("std");

var hd: []i64 = undefined;
var hn: []u32 = undefined;
var hs: usize = 0;

fn push(d: i64, v: u32) void {
    hd[hs] = d;
    hn[hs] = v;
    var i = hs;
    hs += 1;
    while (i > 0 and hd[(i - 1) / 2] > hd[i]) {
        const p = (i - 1) / 2;
        std.mem.swap(i64, &hd[i], &hd[p]);
        std.mem.swap(u32, &hn[i], &hn[p]);
        i = p;
    }
}

fn pop() void {
    hs -= 1;
    hd[0] = hd[hs];
    hn[0] = hn[hs];
    var i: usize = 0;
    while (true) {
        const l = 2 * i + 1;
        if (l >= hs) return;
        const c = if (l + 1 < hs and hd[l + 1] < hd[l]) l + 1 else l;
        if (hd[i] <= hd[c]) return;
        std.mem.swap(i64, &hd[i], &hd[c]);
        std.mem.swap(u32, &hn[i], &hn[c]);
        i = c;
    }
}

pub fn main() !void {
    var pool: std.Io.Threaded = .init(std.heap.smp_allocator, .{});
    defer pool.deinit();
    const gpa = std.heap.smp_allocator;
    var buf: [64 * 1024]u8 = undefined;
    var obuf: [64]u8 = undefined;
    var out = std.Io.File.stdout().writerStreaming(pool.io(), &obuf);
    var file = std.Io.File.stdin().readerStreaming(pool.io(), &buf);
    const text = try file.interface.allocRemaining(gpa, .unlimited);
    var it = std.mem.tokenizeAny(u8, text, " \t\r\n");
    const next = struct {
        fn f(t: *std.mem.TokenIterator(u8, .any)) i64 {
            return std.fmt.parseInt(i64, t.next().?, 10) catch 0;
        }
    }.f;

    const n: usize = @intCast(next(&it));
    const m: usize = @intCast(next(&it));
    const s: usize = @intCast(next(&it));
    const t: usize = @intCast(next(&it));

    const eu = try gpa.alloc(u32, m);
    const ev = try gpa.alloc(u32, m);
    const ew = try gpa.alloc(i64, m);
    for (0..m) |i| {
        eu[i] = @intCast(next(&it));
        ev[i] = @intCast(next(&it));
        ew[i] = next(&it);
    }

    const off = try gpa.alloc(u32, n + 2);
    @memset(off, 0);
    for (0..m) |i| off[eu[i] + 1] += 1;
    for (1..n + 1) |i| off[i] += off[i - 1];
    const cur = try gpa.alloc(u32, n + 2);
    @memcpy(cur, off);
    const to = try gpa.alloc(u32, m);
    const wt = try gpa.alloc(i64, m);
    for (0..m) |i| {
        const p = cur[eu[i]];
        to[p] = ev[i];
        wt[p] = ew[i];
        cur[eu[i]] = p + 1;
    }

    const dist = try gpa.alloc(i64, @max(n, 1));
    @memset(dist, std.math.maxInt(i64));
    dist[s] = 0;
    hd = try gpa.alloc(i64, m + 1);
    hn = try gpa.alloc(u32, m + 1);
    push(0, @intCast(s));
    while (hs > 0) {
        const d = hd[0];
        const u: usize = hn[0];
        pop();
        if (u == t) {
            try out.interface.print("{d}\n", .{d});
            try out.interface.flush();
            return;
        }
        if (d > dist[u]) continue;
        for (off[u]..off[u + 1]) |e| {
            const nd = d + wt[e];
            if (nd < dist[to[e]]) {
                dist[to[e]] = nd;
                push(nd, to[e]);
            }
        }
    }
    try out.interface.print("-1\n", .{});
    try out.interface.flush();
}
