#include <charconv>
#include <cstdio>
#include <cstdlib>
#include <iostream>
#include <string>
#include <utility>
#include <variant>
#include <vector>

struct J;
using Arr = std::vector<J>;
using Obj = std::vector<std::pair<std::string, J>>;
struct J {
  std::variant<std::monostate, bool, long long, std::string, Arr, Obj> v;
};

static std::string src;
static size_t pos = 0;

[[noreturn]] static void die(const char *msg, size_t i) {
  std::printf("error at %zu: %s\n", i + 1, msg);
  std::exit(1);
}

static void ws() {
  while (pos < src.size() && (src[pos] == ' ' || src[pos] == '\t' || src[pos] == '\n' || src[pos] == '\r'))
    pos++;
}

static J lit(const std::string &word, J v) {
  if (src.compare(pos, word.size(), word) == 0) {
    pos += word.size();
    return v;
  }
  die("unexpected character", pos);
}

static J number() {
  size_t start = pos, a = src[pos] == '-' ? pos + 1 : pos, j = a;
  while (j < src.size() && src[j] >= '0' && src[j] <= '9') j++;
  if (j == a) die("unexpected character", start);
  long long n = 0;
  std::from_chars(src.data() + start, src.data() + j, n);
  pos = j;
  return J{n};
}

static std::string text() {
  std::string out;
  for (;;) {
    if (pos >= src.size()) die("unterminated string", pos);
    char c = src[pos++];
    if (c == '"') return out;
    if (c != '\\') {
      out += c;
      continue;
    }
    if (pos >= src.size()) die("unterminated string", pos);
    char e = src[pos++];
    switch (e) {
    case '"': case '\\': case '/': out += e; break;
    case 'n': out += '\n'; break;
    case 't': out += '\t'; break;
    case 'r': out += '\r'; break;
    case 'b': out += '\b'; break;
    case 'f': out += '\f'; break;
    default: die("bad escape", pos - 1);
    }
  }
}

static J value();

static J array() {
  Arr items;
  if (pos < src.size() && src[pos] == ']') {
    pos++;
    return J{std::move(items)};
  }
  for (;;) {
    items.push_back(value());
    ws();
    if (pos >= src.size()) die("expected ',' or ']'", pos);
    if (src[pos] == ',') {
      pos++;
      ws();
    } else if (src[pos] == ']') {
      pos++;
      return J{std::move(items)};
    } else {
      die("expected ',' or ']'", pos);
    }
  }
}

static J object() {
  Obj fields;
  if (pos < src.size() && src[pos] == '}') {
    pos++;
    return J{std::move(fields)};
  }
  for (;;) {
    if (pos >= src.size() || src[pos] != '"') die("unexpected character", pos);
    pos++;
    std::string k = text();
    ws();
    if (pos >= src.size() || src[pos] != ':') die("expected ':'", pos);
    pos++;
    fields.emplace_back(std::move(k), value());
    ws();
    if (pos >= src.size()) die("expected ',' or '}'", pos);
    if (src[pos] == ',') {
      pos++;
      ws();
    } else if (src[pos] == '}') {
      pos++;
      return J{std::move(fields)};
    } else {
      die("expected ',' or '}'", pos);
    }
  }
}

static J value() {
  ws();
  if (pos >= src.size()) die("unexpected end of input", pos);
  switch (src[pos]) {
  case 'n': return lit("null", J{});
  case 't': return lit("true", J{true});
  case 'f': return lit("false", J{false});
  case '"': pos++; return J{text()};
  case '[': pos++; ws(); return array();
  case '{': pos++; ws(); return object();
  default:
    if (src[pos] == '-' || (src[pos] >= '0' && src[pos] <= '9')) return number();
    die("unexpected character", pos);
  }
}

static void quote(std::string &out, const std::string &s) {
  out += '"';
  for (char c : s) switch (c) {
    case '"': out += "\\\""; break;
    case '\\': out += "\\\\"; break;
    case '\n': out += "\\n"; break;
    case '\t': out += "\\t"; break;
    case '\r': out += "\\r"; break;
    case '\b': out += "\\b"; break;
    case '\f': out += "\\f"; break;
    default: out += c;
  }
  out += '"';
}

static void show(std::string &out, const J &j) {
  switch (j.v.index()) {
  case 0: out += "null"; break;
  case 1: out += std::get<bool>(j.v) ? "true" : "false"; break;
  case 2: out += std::to_string(std::get<long long>(j.v)); break;
  case 3: quote(out, std::get<std::string>(j.v)); break;
  case 4: {
    out += '[';
    const Arr &items = std::get<Arr>(j.v);
    for (size_t i = 0; i < items.size(); i++) {
      if (i) out += ',';
      show(out, items[i]);
    }
    out += ']';
    break;
  }
  default: {
    out += '{';
    const Obj &fields = std::get<Obj>(j.v);
    for (size_t i = 0; i < fields.size(); i++) {
      if (i) out += ',';
      quote(out, fields[i].first);
      out += ':';
      show(out, fields[i].second);
    }
    out += '}';
  }
  }
}

int main() {
  std::ios::sync_with_stdio(false);
  std::getline(std::cin, src);
  J v = value();
  ws();
  if (pos < src.size()) die("trailing content", pos);
  std::string out;
  show(out, v);
  out += '\n';
  std::fwrite(out.data(), 1, out.size(), stdout);
}
