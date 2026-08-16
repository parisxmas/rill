const std = @import("std");

pub fn main() !void {
    const w: f64 = 2000;
    const h: f64 = 2000;
    var total: i64 = 0;
    var y: f64 = 0;
    while (y < h) : (y += 1) {
        const ci = (y - h / 2.0) * 4.0 / h;
        var x: f64 = 0;
        while (x < w) : (x += 1) {
            const cr = (x - w / 2.0) * 4.0 / w;
            var zr: f64 = 0;
            var zi: f64 = 0;
            var i: i64 = 0;
            while (i < 255 and zr * zr + zi * zi <= 4.0) {
                const t = zr * zr - zi * zi + cr;
                zi = 2.0 * zr * zi + ci;
                zr = t;
                i += 1;
            }
            total += i;
        }
    }
    var pool: std.Io.Threaded = .init(std.heap.smp_allocator, .{});
    defer pool.deinit();
    var buf: [64]u8 = undefined;
    var out = std.Io.File.stdout().writerStreaming(pool.io(), &buf);
    try out.interface.print("{d}\n", .{total});
    try out.interface.flush();
}
