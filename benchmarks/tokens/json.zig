const std = @import("std");

const Field = struct { k: []const u8, v: *const J };

const J = union(enum) {
    null,
    bool: bool,
    int: i64,
    str: []const u8,
    arr: []const *const J,
    obj: []const Field,
};

var src: []const u8 = "";
var pos: usize = 0;
var gpa: std.mem.Allocator = undefined;
var io: std.Io = undefined;

fn die(msg: []const u8, i: usize) noreturn {
    var buf: [128]u8 = undefined;
    const s = std.fmt.bufPrint(&buf, "error at {d}: {s}\n", .{ i + 1, msg }) catch unreachable;
    std.Io.File.stdout().writeStreamingAll(io, s) catch {};
    std.process.exit(1);
}

fn ws() void {
    while (pos < src.len and (src[pos] == ' ' or src[pos] == '\t' or src[pos] == '\n' or src[pos] == '\r')) pos += 1;
}

fn box(v: J) *const J {
    const p = gpa.create(J) catch unreachable;
    p.* = v;
    return p;
}

fn lit(word: []const u8, v: J) *const J {
    if (pos + word.len <= src.len and std.mem.eql(u8, src[pos .. pos + word.len], word)) {
        pos += word.len;
        return box(v);
    }
    die("unexpected character", pos);
}

fn number() *const J {
    const start = pos;
    const a = if (src[pos] == '-') pos + 1 else pos;
    var j = a;
    while (j < src.len and src[j] >= '0' and src[j] <= '9') j += 1;
    if (j == a) die("unexpected character", start);
    const n = std.fmt.parseInt(i64, src[start..j], 10) catch 0;
    pos = j;
    return box(.{ .int = n });
}

fn text() []const u8 {
    var out: std.ArrayList(u8) = .empty;
    while (true) {
        if (pos >= src.len) die("unterminated string", pos);
        const c = src[pos];
        pos += 1;
        switch (c) {
            '"' => return out.items,
            '\\' => {
                if (pos >= src.len) die("unterminated string", pos);
                const e = src[pos];
                pos += 1;
                out.append(gpa, switch (e) {
                    '"', '\\', '/' => e,
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    'b' => 8,
                    'f' => 12,
                    else => die("bad escape", pos - 1),
                }) catch unreachable;
            },
            else => out.append(gpa, c) catch unreachable,
        }
    }
}

fn value() *const J {
    ws();
    if (pos >= src.len) die("unexpected end of input", pos);
    return switch (src[pos]) {
        'n' => lit("null", .null),
        't' => lit("true", .{ .bool = true }),
        'f' => lit("false", .{ .bool = false }),
        '"' => blk: {
            pos += 1;
            break :blk box(.{ .str = text() });
        },
        '[' => blk: {
            pos += 1;
            ws();
            break :blk array();
        },
        '{' => blk: {
            pos += 1;
            ws();
            break :blk object();
        },
        '-', '0'...'9' => number(),
        else => die("unexpected character", pos),
    };
}

fn array() *const J {
    var items: std.ArrayList(*const J) = .empty;
    if (pos < src.len and src[pos] == ']') {
        pos += 1;
        return box(.{ .arr = items.items });
    }
    while (true) {
        items.append(gpa, value()) catch unreachable;
        ws();
        if (pos >= src.len) die("expected ',' or ']'", pos);
        switch (src[pos]) {
            ',' => {
                pos += 1;
                ws();
            },
            ']' => {
                pos += 1;
                return box(.{ .arr = items.items });
            },
            else => die("expected ',' or ']'", pos),
        }
    }
}

fn object() *const J {
    var fields: std.ArrayList(Field) = .empty;
    if (pos < src.len and src[pos] == '}') {
        pos += 1;
        return box(.{ .obj = fields.items });
    }
    while (true) {
        if (pos >= src.len or src[pos] != '"') die("unexpected character", pos);
        pos += 1;
        const k = text();
        ws();
        if (pos >= src.len or src[pos] != ':') die("expected ':'", pos);
        pos += 1;
        fields.append(gpa, .{ .k = k, .v = value() }) catch unreachable;
        ws();
        if (pos >= src.len) die("expected ',' or '}'", pos);
        switch (src[pos]) {
            ',' => {
                pos += 1;
                ws();
            },
            '}' => {
                pos += 1;
                return box(.{ .obj = fields.items });
            },
            else => die("expected ',' or '}'", pos),
        }
    }
}

fn quote(out: *std.ArrayList(u8), s: []const u8) void {
    out.append(gpa, '"') catch unreachable;
    for (s) |c| {
        const esc: ?[]const u8 = switch (c) {
            '"' => "\\\"",
            '\\' => "\\\\",
            '\n' => "\\n",
            '\t' => "\\t",
            '\r' => "\\r",
            8 => "\\b",
            12 => "\\f",
            else => null,
        };
        if (esc) |e| out.appendSlice(gpa, e) catch unreachable else out.append(gpa, c) catch unreachable;
    }
    out.append(gpa, '"') catch unreachable;
}

fn show(out: *std.ArrayList(u8), v: *const J) void {
    switch (v.*) {
        .null => out.appendSlice(gpa, "null") catch unreachable,
        .bool => |b| out.appendSlice(gpa, if (b) "true" else "false") catch unreachable,
        .int => |n| out.print(gpa, "{d}", .{n}) catch unreachable,
        .str => |s| quote(out, s),
        .arr => |items| {
            out.append(gpa, '[') catch unreachable;
            for (items, 0..) |e, i| {
                if (i > 0) out.append(gpa, ',') catch unreachable;
                show(out, e);
            }
            out.append(gpa, ']') catch unreachable;
        },
        .obj => |fields| {
            out.append(gpa, '{') catch unreachable;
            for (fields, 0..) |f, i| {
                if (i > 0) out.append(gpa, ',') catch unreachable;
                quote(out, f.k);
                out.append(gpa, ':') catch unreachable;
                show(out, f.v);
            }
            out.append(gpa, '}') catch unreachable;
        },
    }
}

pub fn main() !void {
    var pool: std.Io.Threaded = .init(std.heap.smp_allocator, .{});
    defer pool.deinit();
    gpa = std.heap.smp_allocator;
    io = pool.io();

    var buf: [64 * 1024]u8 = undefined;
    var in = std.Io.File.stdin().readerStreaming(io, &buf);
    const all = try in.interface.allocRemaining(gpa, .unlimited);
    src = std.mem.trimEnd(u8, all, "\n");

    const v = value();
    ws();
    if (pos < src.len) die("trailing content", pos);

    var out: std.ArrayList(u8) = .empty;
    show(&out, v);
    out.append(gpa, '\n') catch unreachable;
    try std.Io.File.stdout().writeStreamingAll(io, out.items);
}
