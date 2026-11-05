//! A template's `theme.luau` read as data: `return` and a Luau table
//! literal of strings, numbers, booleans and tables, as a palette is written.
//! Anything else (code: a call, a variable, an operator) isn't data, and
//! reads as nothing. Nothing here runs Luau.

use serde_json::{Map, Value as Json};

/// The most of a file read: a theme is a few kilobytes.
pub const MAX_BYTES: usize = 256 * 1024;

/// `text` as data, or None for anything that isn't (code, or too long).
pub fn parse(text: &str) -> Option<Json> {
    if text.len() > MAX_BYTES {
        return None;
    }
    let mut p = Parser { s: text.as_bytes(), at: 0 };
    p.space();
    if !p.word("return") {
        return None;
    }
    let value = p.value()?;
    p.space();
    (p.at == p.s.len()).then_some(value)
}

struct Parser<'a> {
    s: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.at).copied()
    }

    /// Past white space and `--` comments (a line's, or a `--[[ ]]` block).
    fn space(&mut self) {
        loop {
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.at += 1;
            }
            if self.s[self.at..].starts_with(b"--[[") {
                match self.s[self.at..].windows(2).position(|w| w == b"]]") {
                    Some(end) => self.at += end + 2,
                    None => self.at = self.s.len(),
                }
            } else if self.s[self.at..].starts_with(b"--") {
                while self.peek().is_some_and(|c| c != b'\n') {
                    self.at += 1;
                }
            } else {
                return;
            }
        }
    }

    fn word(&mut self, word: &str) -> bool {
        self.space();
        let end = self.at + word.len();
        let next = self.s.get(end).copied();
        if self.s[self.at..].starts_with(word.as_bytes()) && !next.is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
            self.at = end;
            return true;
        }
        false
    }

    fn name(&mut self) -> Option<String> {
        self.space();
        let start = self.at;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
            self.at += 1;
        }
        (self.at > start && !self.s[start].is_ascii_digit()).then(|| String::from_utf8_lossy(&self.s[start..self.at]).into_owned())
    }

    fn string(&mut self) -> Option<String> {
        self.space();
        let quote = self.peek().filter(|c| *c == b'"' || *c == b'\'')?;
        self.at += 1;
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            self.at += 1;
            match c {
                b'\\' => {
                    let e = self.peek()?;
                    self.at += 1;
                    out.push(match e {
                        b'n' => b'\n',
                        b't' => b'\t',
                        other => other,
                    });
                }
                c if c == quote => return String::from_utf8(out).ok(),
                b'\n' => return None,
                c => out.push(c),
            }
        }
    }

    fn value(&mut self) -> Option<Json> {
        self.space();
        match self.peek()? {
            b'"' | b'\'' => self.string().map(Json::String),
            b'{' => self.table(),
            c if c == b'-' || c.is_ascii_digit() => {
                let start = self.at;
                self.at += 1;
                while self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'.') {
                    self.at += 1;
                }
                std::str::from_utf8(&self.s[start..self.at]).ok()?.parse::<f64>().ok().and_then(|n| serde_json::Number::from_f64(n).map(Json::Number))
            }
            _ => {
                if self.word("true") {
                    Some(Json::Bool(true))
                } else if self.word("false") {
                    Some(Json::Bool(false))
                } else if self.word("nil") {
                    Some(Json::Null)
                } else {
                    None
                }
            }
        }
    }

    /// `{ name = v, ["key"] = v, v, … }`: an object when it has a key, else
    /// a list.
    fn table(&mut self) -> Option<Json> {
        self.at += 1;
        let mut map = Map::new();
        let mut list = Vec::new();
        loop {
            self.space();
            match self.peek()? {
                b'}' => {
                    self.at += 1;
                    break;
                }
                b'[' => {
                    self.at += 1;
                    let key = self.string()?;
                    self.space();
                    (self.peek()? == b']').then_some(())?;
                    self.at += 1;
                    self.equals()?;
                    map.insert(key, self.value()?);
                }
                _ => {
                    let save = self.at;
                    match self.name() {
                        Some(key) if !matches!(key.as_str(), "true" | "false" | "nil") && self.equals().is_some() => {
                            map.insert(key, self.value()?);
                        }
                        _ => {
                            self.at = save;
                            list.push(self.value()?);
                        }
                    }
                }
            }
            self.space();
            if matches!(self.peek(), Some(b',' | b';')) {
                self.at += 1;
            }
        }
        match (map.is_empty(), list.is_empty()) {
            (_, true) => Some(Json::Object(map)),
            (true, false) => Some(Json::Array(list)),
            (false, false) => None,
        }
    }

    fn equals(&mut self) -> Option<()> {
        self.space();
        (self.peek()? == b'=' && self.s.get(self.at + 1) != Some(&b'=')).then(|| self.at += 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_theme_file_reads_as_data_and_code_does_not() {
        let v = parse("-- a palette\nreturn {\n\tname = \"Mine\", -- its name\n\tdark = { text = 'a\\'b', syntax = { \"#1\", \"#2\" }, n = 1.5, on = true },\n\t[\"light\"] = nil,\n}\n").unwrap();
        assert_eq!(v["name"], "Mine");
        assert_eq!(v["dark"]["text"], "a'b");
        assert_eq!(v["dark"]["syntax"], json!(["#1", "#2"]));
        assert_eq!(v["dark"]["n"], 1.5);
        assert_eq!(parse("return nil\n"), Some(Json::Null));
        assert!(parse("local x = 1\nreturn { a = x }").is_none());
        assert!(parse("return require(\"@ds/thing\")").is_none());
        assert!(parse("return { a = \"x\" } extra").is_none());
    }
}
