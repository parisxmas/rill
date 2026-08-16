package main

import (
	"bufio"
	"fmt"
	"os"
	"strconv"
	"strings"
)

type Field struct {
	K string
	V any
}

var src string
var pos int

func die(msg string, i int) {
	fmt.Printf("error at %d: %s\n", i+1, msg)
	os.Exit(1)
}

func ws() {
	for pos < len(src) && (src[pos] == ' ' || src[pos] == '\t' || src[pos] == '\n' || src[pos] == '\r') {
		pos++
	}
}

func plit(lit string, v any) any {
	if pos+len(lit) <= len(src) && src[pos:pos+len(lit)] == lit {
		pos += len(lit)
		return v
	}
	die("unexpected character", pos)
	return nil
}

func pint() any {
	i := pos
	a := i
	if src[pos] == '-' {
		a = i + 1
	}
	j := a
	for j < len(src) && src[j] >= '0' && src[j] <= '9' {
		j++
	}
	if j == a {
		die("unexpected character", i)
	}
	n, _ := strconv.ParseInt(src[i:j], 10, 64)
	pos = j
	return n
}

func pstr() string {
	var b strings.Builder
	for {
		if pos >= len(src) {
			die("unterminated string", pos)
		}
		c := src[pos]
		if c == '"' {
			pos++
			return b.String()
		}
		if c == '\\' {
			pos++
			if pos >= len(src) {
				die("unterminated string", pos)
			}
			switch src[pos] {
			case '"', '\\', '/':
				b.WriteByte(src[pos])
			case 'n':
				b.WriteByte('\n')
			case 't':
				b.WriteByte('\t')
			case 'r':
				b.WriteByte('\r')
			case 'b':
				b.WriteByte('\b')
			case 'f':
				b.WriteByte('\f')
			default:
				die("bad escape", pos)
			}
			pos++
			continue
		}
		b.WriteByte(c)
		pos++
	}
}

func parse() any {
	ws()
	if pos >= len(src) {
		die("unexpected end of input", pos)
	}
	c := src[pos]
	switch {
	case c == 'n':
		return plit("null", nil)
	case c == 't':
		return plit("true", true)
	case c == 'f':
		return plit("false", false)
	case c == '"':
		pos++
		return pstr()
	case c == '[':
		pos++
		ws()
		return parr()
	case c == '{':
		pos++
		ws()
		return pobj()
	case c == '-' || (c >= '0' && c <= '9'):
		return pint()
	}
	die("unexpected character", pos)
	return nil
}

func parr() any {
	items := []any{}
	if pos < len(src) && src[pos] == ']' {
		pos++
		return items
	}
	for {
		items = append(items, parse())
		ws()
		if pos >= len(src) {
			die("expected ',' or ']'", pos)
		}
		if src[pos] == ',' {
			pos++
			ws()
			continue
		}
		if src[pos] == ']' {
			pos++
			return items
		}
		die("expected ',' or ']'", pos)
	}
}

func pobj() any {
	fields := []Field{}
	if pos < len(src) && src[pos] == '}' {
		pos++
		return fields
	}
	for {
		if pos >= len(src) || src[pos] != '"' {
			die("unexpected character", pos)
		}
		pos++
		k := pstr()
		ws()
		if pos >= len(src) || src[pos] != ':' {
			die("expected ':'", pos)
		}
		pos++
		fields = append(fields, Field{k, parse()})
		ws()
		if pos >= len(src) {
			die("expected ',' or '}'", pos)
		}
		if src[pos] == ',' {
			pos++
			ws()
			continue
		}
		if src[pos] == '}' {
			pos++
			return fields
		}
		die("expected ',' or '}'", pos)
	}
}

func quote(s string) string {
	var b strings.Builder
	b.WriteByte('"')
	for i := 0; i < len(s); i++ {
		switch c := s[i]; c {
		case '"':
			b.WriteString("\\\"")
		case '\\':
			b.WriteString("\\\\")
		case '\n':
			b.WriteString("\\n")
		case '\t':
			b.WriteString("\\t")
		case '\r':
			b.WriteString("\\r")
		case '\b':
			b.WriteString("\\b")
		case '\f':
			b.WriteString("\\f")
		default:
			b.WriteByte(c)
		}
	}
	b.WriteByte('"')
	return b.String()
}

func show(v any) string {
	switch x := v.(type) {
	case nil:
		return "null"
	case bool:
		if x {
			return "true"
		}
		return "false"
	case int64:
		return strconv.FormatInt(x, 10)
	case string:
		return quote(x)
	case []any:
		parts := make([]string, len(x))
		for i, e := range x {
			parts[i] = show(e)
		}
		return "[" + strings.Join(parts, ",") + "]"
	case []Field:
		parts := make([]string, len(x))
		for i, f := range x {
			parts[i] = quote(f.K) + ":" + show(f.V)
		}
		return "{" + strings.Join(parts, ",") + "}"
	}
	return ""
}

func main() {
	line, _ := bufio.NewReader(os.Stdin).ReadString('\n')
	src = strings.TrimRight(line, "\n")
	v := parse()
	ws()
	if pos < len(src) {
		die("trailing content", pos)
	}
	fmt.Println(show(v))
}
