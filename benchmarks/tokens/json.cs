using System;
using System.Collections.Generic;
using System.Text;

abstract record J;
sealed record JNull : J;
sealed record JBool(bool V) : J;
sealed record JInt(long V) : J;
sealed record JStr(string V) : J;
sealed record JArr(List<J> Items) : J;
sealed record JObj(List<(string K, J V)> Fields) : J;

static class Json {
    static string src = "";
    static int pos;

    static T Die<T>(string msg, int i) {
        Console.WriteLine($"error at {i + 1}: {msg}");
        Environment.Exit(1);
        return default!;
    }

    static void Ws() {
        while (pos < src.Length && (src[pos] == ' ' || src[pos] == '\t' || src[pos] == '\n' || src[pos] == '\r')) pos++;
    }

    static J Lit(string lit, J v) {
        if (pos + lit.Length <= src.Length && string.CompareOrdinal(src, pos, lit, 0, lit.Length) == 0) {
            pos += lit.Length;
            return v;
        }
        return Die<J>("unexpected character", pos);
    }

    static J Number() {
        int start = pos;
        int a = src[pos] == '-' ? pos + 1 : pos;
        int j = a;
        while (j < src.Length && src[j] >= '0' && src[j] <= '9') j++;
        if (j == a) Die<J>("unexpected character", start);
        long n = long.Parse(src.AsSpan(start, j - start));
        pos = j;
        return new JInt(n);
    }

    static string Text() {
        var b = new StringBuilder();
        while (true) {
            if (pos >= src.Length) Die<J>("unterminated string", pos);
            char c = src[pos++];
            if (c == '"') return b.ToString();
            if (c != '\\') { b.Append(c); continue; }
            if (pos >= src.Length) Die<J>("unterminated string", pos);
            char e = src[pos++];
            b.Append(e switch {
                '"' or '\\' or '/' => e,
                'n' => '\n',
                't' => '\t',
                'r' => '\r',
                'b' => '\b',
                'f' => '\f',
                _ => Die<char>("bad escape", pos - 1),
            });
        }
    }

    static J Value() {
        Ws();
        if (pos >= src.Length) Die<J>("unexpected end of input", pos);
        char c = src[pos];
        if (c == 'n') return Lit("null", new JNull());
        if (c == 't') return Lit("true", new JBool(true));
        if (c == 'f') return Lit("false", new JBool(false));
        if (c == '"') { pos++; return new JStr(Text()); }
        if (c == '[') { pos++; Ws(); return Array(); }
        if (c == '{') { pos++; Ws(); return Object(); }
        if (c == '-' || (c >= '0' && c <= '9')) return Number();
        return Die<J>("unexpected character", pos);
    }

    static J Array() {
        var items = new List<J>();
        if (pos < src.Length && src[pos] == ']') { pos++; return new JArr(items); }
        while (true) {
            items.Add(Value());
            Ws();
            if (pos >= src.Length) Die<J>("expected ',' or ']'", pos);
            if (src[pos] == ',') { pos++; Ws(); continue; }
            if (src[pos] == ']') { pos++; return new JArr(items); }
            Die<J>("expected ',' or ']'", pos);
        }
    }

    static J Object() {
        var fields = new List<(string, J)>();
        if (pos < src.Length && src[pos] == '}') { pos++; return new JObj(fields); }
        while (true) {
            if (pos >= src.Length || src[pos] != '"') Die<J>("unexpected character", pos);
            pos++;
            string k = Text();
            Ws();
            if (pos >= src.Length || src[pos] != ':') Die<J>("expected ':'", pos);
            pos++;
            fields.Add((k, Value()));
            Ws();
            if (pos >= src.Length) Die<J>("expected ',' or '}'", pos);
            if (src[pos] == ',') { pos++; Ws(); continue; }
            if (src[pos] == '}') { pos++; return new JObj(fields); }
            Die<J>("expected ',' or '}'", pos);
        }
    }

    static void Quote(StringBuilder b, string s) {
        b.Append('"');
        foreach (char c in s)
            switch (c) {
                case '"': b.Append("\\\""); break;
                case '\\': b.Append("\\\\"); break;
                case '\n': b.Append("\\n"); break;
                case '\t': b.Append("\\t"); break;
                case '\r': b.Append("\\r"); break;
                case '\b': b.Append("\\b"); break;
                case '\f': b.Append("\\f"); break;
                default: b.Append(c); break;
            }
        b.Append('"');
    }

    static void Show(StringBuilder b, J v) {
        switch (v) {
            case JNull: b.Append("null"); break;
            case JBool x: b.Append(x.V ? "true" : "false"); break;
            case JInt x: b.Append(x.V); break;
            case JStr x: Quote(b, x.V); break;
            case JArr x:
                b.Append('[');
                for (int i = 0; i < x.Items.Count; i++) { if (i > 0) b.Append(','); Show(b, x.Items[i]); }
                b.Append(']');
                break;
            case JObj x:
                b.Append('{');
                for (int i = 0; i < x.Fields.Count; i++) {
                    if (i > 0) b.Append(',');
                    Quote(b, x.Fields[i].K);
                    b.Append(':');
                    Show(b, x.Fields[i].V);
                }
                b.Append('}');
                break;
        }
    }

    static void Main() {
        src = (Console.In.ReadLine() ?? "");
        J v = Value();
        Ws();
        if (pos < src.Length) Die<J>("trailing content", pos);
        var b = new StringBuilder();
        Show(b, v);
        var w = new System.IO.StreamWriter(Console.OpenStandardOutput());
        w.WriteLine(b.ToString());
        w.Flush();
    }
}
