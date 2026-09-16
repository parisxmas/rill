// Rill coloured: a tokenizer for the code view, small enough to read.
// Comments, strings with their escapes, numbers, the keywords, the
// machine's own words where they are keywords, constructors and types by
// their capital, a name before `(` as a call. The same classes the VS Code
// grammar names, so a theme written for one serves the other.

const Highlight = (() => {
  const KEYWORDS = new Set([
    "fn", "type", "trait", "impl", "extern", "asm", "naked", "export", "private", "where", "module", "import", "as",
    "if", "then", "else", "match", "for", "in", "with", "while", "spawn", "select", "timeout", "default", "blocking",
    "machine", "state", "after", "save", "when", "priority", "system", "stay", "stop", "done",
  ]);
  const CONSTANTS = new Set(["true", "false", "None", "Some", "Ok", "Err", "Nil", "Cons"]);
  const BUILTIN_TYPES = new Set(["Int", "Float", "Bool", "Str", "Unit", "Ptr", "Chan", "Buf", "Map", "Mail", "Option", "Result", "List", "Show", "Ord"]);
  const MACHINE_WORDS = new Set(["sender", "reply", "output", "post", "set", "reset", "active", "me"]);

  const esc = s => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

  // One line as HTML spans. A line is self-contained in Rill: no comment or
  // string crosses a line end, which is what makes this a line at a time.
  function line(text) {
    let out = "", i = 0;
    const n = text.length;
    while (i < n) {
      const c = text[i];
      if (c === "#") {
        out += `<span class="tk-comment">${esc(text.slice(i))}</span>`;
        break;
      }
      if (c === '"') {
        let j = i + 1, body = "";
        while (j < n && text[j] !== '"') {
          if (text[j] === "\\" && j + 1 < n) {
            body += `<span class="tk-escape">${esc(text.slice(j, j + 2))}</span>`;
            j += 2;
          } else {
            body += esc(text[j]);
            j++;
          }
        }
        out += `<span class="tk-string">"${body}${j < n ? '"' : ""}</span>`;
        i = j + 1;
        continue;
      }
      if (/[0-9]/.test(c) && (i === 0 || !/[A-Za-z0-9_]/.test(text[i - 1]))) {
        let j = i;
        while (j < n && /[0-9]/.test(text[j])) j++;
        if (text[j] === "." && /[0-9]/.test(text[j + 1] || "")) { j++; while (j < n && /[0-9]/.test(text[j])) j++; }
        if (text[j] === "e" || text[j] === "E") { let k = j + 1; if (text[k] === "+" || text[k] === "-") k++; if (/[0-9]/.test(text[k] || "")) { j = k; while (j < n && /[0-9]/.test(text[j])) j++; } }
        out += `<span class="tk-number">${text.slice(i, j)}</span>`;
        i = j;
        continue;
      }
      if (/[A-Za-z_]/.test(c)) {
        let j = i;
        while (j < n && /[A-Za-z0-9_]/.test(text[j])) j++;
        const word = text.slice(i, j);
        const next = text.slice(j).match(/^\s*(\S)/);
        const after = next ? next[1] : "";
        let cls = "";
        if (KEYWORDS.has(word)) cls = "tk-keyword";
        else if (CONSTANTS.has(word)) cls = "tk-constant";
        else if (BUILTIN_TYPES.has(word)) cls = "tk-type";
        else if (/^[A-Z]/.test(word)) cls = "tk-ctor";
        else if (MACHINE_WORDS.has(word)) cls = "tk-machine";
        else if (after === "(" && !(i > 0 && text[i - 1] === ".")) cls = "tk-call";
        else if (word === "_") cls = "tk-wild";
        out += cls ? `<span class="${cls}">${word}</span>` : word;
        i = j;
        continue;
      }
      if (c === "-" && text[i + 1] === ">") { out += `<span class="tk-arrow">-&gt;</span>`; i += 2; continue; }
      if (c === "|" && text[i + 1] === ">") { out += `<span class="tk-op">|&gt;</span>`; i += 2; continue; }
      if (c === "\\") { out += `<span class="tk-lambda">\\</span>`; i++; continue; }
      if ("+-*/%=<>!&|?".includes(c)) { out += `<span class="tk-op">${esc(c)}</span>`; i++; continue; }
      out += esc(c);
      i++;
    }
    return out;
  }

  function html(source) {
    return source.split("\n").map(line).join("\n");
  }

  return { html, line };
})();
