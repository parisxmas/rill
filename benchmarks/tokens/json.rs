use std::io::Write;

enum J {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<J>),
    Obj(Vec<(String, J)>),
}

struct P {
    s: Vec<u8>,
    i: usize,
}

fn die(msg: &str, i: usize) -> ! {
    println!("error at {}: {}", i + 1, msg);
    std::process::exit(1);
}

impl P {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn lit(&mut self, lit: &str, v: J) -> J {
        let n = lit.len();
        if self.i + n <= self.s.len() && &self.s[self.i..self.i + n] == lit.as_bytes() {
            self.i += n;
            return v;
        }
        die("unexpected character", self.i)
    }

    fn int(&mut self) -> J {
        let start = self.i;
        let a = if self.s[self.i] == b'-' { self.i + 1 } else { self.i };
        let mut j = a;
        while j < self.s.len() && self.s[j].is_ascii_digit() {
            j += 1;
        }
        if j == a {
            die("unexpected character", start);
        }
        let n: i64 = std::str::from_utf8(&self.s[start..j]).unwrap().parse().unwrap();
        self.i = j;
        J::Int(n)
    }

    fn text(&mut self) -> String {
        let mut out = Vec::new();
        loop {
            if self.i >= self.s.len() {
                die("unterminated string", self.i);
            }
            let c = self.s[self.i];
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).unwrap(),
                b'\\' => {
                    if self.i >= self.s.len() {
                        die("unterminated string", self.i);
                    }
                    let e = self.s[self.i];
                    self.i += 1;
                    out.push(match e {
                        b'"' | b'\\' | b'/' => e,
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        b'b' => 8,
                        b'f' => 12,
                        _ => die("bad escape", self.i - 1),
                    });
                }
                _ => out.push(c),
            }
        }
    }

    fn value(&mut self) -> J {
        self.ws();
        if self.i >= self.s.len() {
            die("unexpected end of input", self.i);
        }
        match self.s[self.i] {
            b'n' => self.lit("null", J::Null),
            b't' => self.lit("true", J::Bool(true)),
            b'f' => self.lit("false", J::Bool(false)),
            b'"' => {
                self.i += 1;
                J::Str(self.text())
            }
            b'[' => {
                self.i += 1;
                self.ws();
                self.array()
            }
            b'{' => {
                self.i += 1;
                self.ws();
                self.object()
            }
            b'-' | b'0'..=b'9' => self.int(),
            _ => die("unexpected character", self.i),
        }
    }

    fn array(&mut self) -> J {
        let mut items = Vec::new();
        if self.i < self.s.len() && self.s[self.i] == b']' {
            self.i += 1;
            return J::Arr(items);
        }
        loop {
            items.push(self.value());
            self.ws();
            if self.i >= self.s.len() {
                die("expected ',' or ']'", self.i);
            }
            match self.s[self.i] {
                b',' => {
                    self.i += 1;
                    self.ws();
                }
                b']' => {
                    self.i += 1;
                    return J::Arr(items);
                }
                _ => die("expected ',' or ']'", self.i),
            }
        }
    }

    fn object(&mut self) -> J {
        let mut fields = Vec::new();
        if self.i < self.s.len() && self.s[self.i] == b'}' {
            self.i += 1;
            return J::Obj(fields);
        }
        loop {
            if self.i >= self.s.len() || self.s[self.i] != b'"' {
                die("unexpected character", self.i);
            }
            self.i += 1;
            let k = self.text();
            self.ws();
            if self.i >= self.s.len() || self.s[self.i] != b':' {
                die("expected ':'", self.i);
            }
            self.i += 1;
            fields.push((k, self.value()));
            self.ws();
            if self.i >= self.s.len() {
                die("expected ',' or '}'", self.i);
            }
            match self.s[self.i] {
                b',' => {
                    self.i += 1;
                    self.ws();
                }
                b'}' => {
                    self.i += 1;
                    return J::Obj(fields);
                }
                _ => die("expected ',' or '}'", self.i),
            }
        }
    }
}

fn quote(out: &mut Vec<u8>, s: &str) {
    out.push(b'"');
    for c in s.bytes() {
        match c {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\r' => out.extend_from_slice(b"\\r"),
            8 => out.extend_from_slice(b"\\b"),
            12 => out.extend_from_slice(b"\\f"),
            _ => out.push(c),
        }
    }
    out.push(b'"');
}

fn show(out: &mut Vec<u8>, v: &J) {
    match v {
        J::Null => out.extend_from_slice(b"null"),
        J::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        J::Int(n) => out.extend_from_slice(n.to_string().as_bytes()),
        J::Str(s) => quote(out, s),
        J::Arr(items) => {
            out.push(b'[');
            for (i, e) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                show(out, e);
            }
            out.push(b']');
        }
        J::Obj(fields) => {
            out.push(b'{');
            for (i, (k, e)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                quote(out, k);
                out.push(b':');
                show(out, e);
            }
            out.push(b'}');
        }
    }
}

fn main() {
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    let mut p = P { s: line.trim_end_matches('\n').as_bytes().to_vec(), i: 0 };
    let v = p.value();
    p.ws();
    if p.i < p.s.len() {
        die("trailing content", p.i);
    }
    let mut out = Vec::new();
    show(&mut out, &v);
    out.push(b'\n');
    std::io::stdout().write_all(&out).unwrap();
}
