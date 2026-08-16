const std = @import("std");

pub fn main() !void {
    var v: [1024]i64 = undefined;
    for (&v, 0..) |*x, i| x.* = @intCast(i);
    var acc: i64 = 0;
    var i: i64 = 0;
    while (i < 100_000_000) : (i += 1) acc += v[@intCast(i & 1023)];
    var buf: [64]u8 = undefined;
    var pool: std.Io.Threaded = .init(std.heap.smp_allocator, .{});
    defer pool.deinit();
    var out = std.Io.File.stdout().writerStreaming(pool.io(), &buf);
    try out.interface.print("{d}\n", .{acc});
    try out.interface.flush();
}
